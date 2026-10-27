//! Folder projects: a folder on this computer opened as a LibrePaper project.
//!
//! The folder is the project. This module opens one through the native
//! chooser, lists the ones this origin has opened, reads a folder's tree and
//! files, and reports changes other programs make to it. Edits travel the
//! other way through `PUT folder`, which already mirrors a project into a
//! granted folder.
//!
//! The tree route takes an optional `extensions` query value, a comma-separated
//! list such as `tex,typ,md`. A file whose extension is not in it is never
//! read, listed or named in `skipped`, and does not count toward
//! `MAX_PROJECT_FILES`, so a Downloads folder opens without hashing its videos.
//! Without the value every file is listed. Digests come from a cache keyed by
//! path and valid while the file's size and modification time hold, which the
//! folder save and the render share, so only a changed file is read again. The
//! walk also stops after `MAX_WALK_ENTRIES` directory entries.
//!
//! Outside changes are found by polling. One background task per polled
//! project stats the tree every second, so the same code runs on every
//! platform and on network drives. The poller looks only at the extensions the
//! tree was read with. A file is reported once its size and
//! modification time held still across one poll, and a change whose digest
//! matches the written record is LibrePaper's own write and is not reported.
//!
//! A render in a folder project writes its own files into the folder: the
//! result, the SyncTeX file and whatever the tool leaves beside them. Those
//! are not outside changes. Importing one would change the project, which
//! renders again, and an output that differs on every run would loop forever.
//! So the job worker records the paths a render wrote, per project, and the
//! poller and the tree listing leave them out for as long as they stay
//! recorded. The record lives in memory, so a restart empties it until the
//! next render in that project.
//!
//! - The result and SyncTeX paths are recorded before the command starts.
//! - Every other file the run created or changed, found by listing the folder
//!   before and after it, is recorded when it ends, but only when it is named
//!   for the result: it sits in the result's directory or below it, and the
//!   first name there is the result's stem followed by `.`, `_` or `-`. For
//!   `main.pdf` that takes `main.aux`, `main_files/libs/x.css` and
//!   `main-figure0.pdf`, and leaves `refs.bib`, `data/x.csv` and
//!   `chapters/one.tex` alone. A file another program writes while the render
//!   runs, such as `refs.bib` arriving through Dropbox, is never claimed,
//!   because claiming it would silence every later change to it. A file the
//!   project itself lists is the editor's and is never claimed either, so a
//!   command that rewrites a source file still shows up as an outside change.
//! - While the command runs, a new file named for the result is held back as
//!   well, because the run may write it long before it ends and `finish` has
//!   not recorded it yet. A path the page already holds is the editor's and is
//!   reported as usual. The running state ends when the watch does, however the
//!   run ended, and only after the outputs are recorded.
//! - At most `MAX_RENDER_OUTPUTS` paths are kept per project, and forgetting
//!   the project drops them. A poller that stops for want of requests keeps
//!   them, because the folder's outputs are still not the person's files when
//!   the project is opened again.
//! - A file the page holds that a render then records is the render's from
//!   then on. The page keeps its copy while the run goes on. When the run ends,
//!   `finish` forgets the file in the folder's written record without moving
//!   anything, and the poller reports the file removed so the page drops it. A
//!   later save that omits it then trashes nothing. A page whose poller has
//!   stopped is told to read the tree again, which no longer lists the file.
//!
//! A file is reported removed only when its absence is confirmed. The folder
//! can fail to answer for a moment, through permissions or a network drive,
//! and the page would drop every file under a directory it could not read, and
//! the next save would trash them. So a held file that the walk did not find is
//! reported removed only when no skipped entry is it or holds it and the
//! operating system says it is not there (`NotFound`, `NotADirectory`) or that
//! a directory took its place. Anything else leaves it as it is.

use super::*;
use librepaper_base::paths::RelPath;

use std::collections::BTreeSet;
use std::io::ErrorKind;
use std::time::UNIX_EPOCH;

use crate::local::bindings::ProjectBinding;
use crate::local::render::workspace;

/// Most files a folder project may list. Only files that match the request's
/// extensions count. A bigger folder answers 413.
pub(super) const MAX_PROJECT_FILES: usize = 50_000;

/// Most directory entries a walk visits, matched or not, so that opening a
/// home directory stops instead of walking it for as long as it takes. Past
/// it the walk fails like a folder with too many files.
const MAX_WALK_ENTRIES: usize = 1_000_000;

/// Most bytes one entry of a folder save's manifest may take as JSON. An entry
/// is `{"path":"...","sha256":"...","size":N},`, which is about 110 bytes of
/// syntax, digest and size around the path. 512 bytes leave room for paths of
/// 400 bytes and more.
const MANIFEST_ENTRY_BYTES: usize = 512;

/// Most bytes the manifest of a folder save may take: every file a folder
/// project may list, at `MANIFEST_ENTRY_BYTES` each, which is 24.4 MiB. The
/// fixed bound for other JSON, 64 KiB, holds a few hundred files, and a save
/// above that answered 413 even with no bytes to upload. The description of a
/// render job carries the same manifest, so it takes the same bound.
pub(super) const MAX_FOLDER_MANIFEST_BYTES: usize = MAX_PROJECT_FILES * MANIFEST_ENTRY_BYTES;

/// Most render outputs remembered per project. Once the record is full, later
/// paths are not added; the ones it holds stay.
const MAX_RENDER_OUTPUTS: usize = 512;

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
pub(super) fn skip_reason(name: &str) -> Option<&'static str> {
    if name.starts_with('.') {
        Some("hidden")
    } else if IGNORED_NAMES.contains(&name) {
        Some("ignored")
    } else {
        None
    }
}

/// The extensions a request asks for: lowercase, without dots.
pub(super) type Extensions = BTreeSet<String>;

/// Parse the comma-separated `extensions` query value. Blank entries and
/// leading dots are dropped. A value with no extension in it is no filter.
fn parse_extensions(value: &str) -> Option<Extensions> {
    let set: Extensions = value
        .split(',')
        .map(|part| part.trim().trim_start_matches('.').to_ascii_lowercase())
        .filter(|part| !part.is_empty())
        .collect();
    (!set.is_empty()).then_some(set)
}

/// Whether a file called `name` is wanted. Without a filter every file is. With
/// one, the part after the last `.` must be in it, ignoring case, and a name
/// without a `.` never matches.
fn extension_matches(filter: Option<&Extensions>, name: &str) -> bool {
    let Some(filter) = filter else {
        return true;
    };
    name.rsplit_once('.')
        .is_some_and(|(_, extension)| filter.contains(&extension.to_ascii_lowercase()))
}

/// Walk `root` and list its regular files. Hidden entries, the ignored
/// names, symlinks, files over the upload bound and anything that is not a
/// regular file are skipped. More than `MAX_PROJECT_FILES` files, or more than
/// `MAX_WALK_ENTRIES` directory entries, is an error.
pub(super) fn walk(root: &Path) -> Result<Walk, WalkError> {
    walk_matching(root, None)
}

/// Like `walk`, but with a filter a regular file whose extension is not in it
/// is left out silently: it is not listed, not skipped and not counted toward
/// the file cap. Directories and everything else are handled as `walk` does.
fn walk_matching(root: &Path, filter: Option<&Extensions>) -> Result<Walk, WalkError> {
    walk_limited(root, filter, MAX_PROJECT_FILES, MAX_WALK_ENTRIES)
}

fn walk_limited(
    root: &Path,
    filter: Option<&Extensions>,
    max_files: usize,
    max_entries: usize,
) -> Result<Walk, WalkError> {
    let mut result = Walk::default();
    let mut visited = 0usize;
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
            visited += 1;
            if visited > max_entries {
                return Err(WalkError::TooMany);
            }
            let Some(name) = entry.file_name().to_str().map(str::to_string) else {
                continue;
            };
            let kind = entry.file_type();
            if kind.as_ref().is_ok_and(|kind| kind.is_file()) && !extension_matches(filter, &name) {
                continue;
            }
            let relative = if prefix.is_empty() {
                name.clone()
            } else {
                format!("{prefix}/{name}")
            };
            if let Some(reason) = skip_reason(&name) {
                result.skipped.push((relative, reason));
                continue;
            }
            let Ok(kind) = kind else {
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
                } else if !RelPath::is_valid(&relative) {
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
                    if result.files.len() > max_files {
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

/// Whether `entry`, a path the walk skipped, is `path` or a folder holding it.
fn covers(entry: &str, path: &str) -> bool {
    path.strip_prefix(entry)
        .is_some_and(|rest| rest.is_empty() || rest.starts_with('/'))
}

/// Whether `path`, a file the walk did not list, is gone for sure. It is not
/// when a skipped entry is it or holds it, since the walk could not look
/// there, and not when the operating system fails to answer for another reason
/// than the path being absent, such as a permission or a drive that stopped
/// responding. A directory in its place means the file is gone.
fn confirmed_gone(root: &Path, path: &str, skipped: &[(String, &'static str)]) -> bool {
    if skipped.iter().any(|(entry, _)| covers(entry, path)) {
        return false;
    }
    match std::fs::symlink_metadata(root.join(path)) {
        Ok(metadata) => metadata.is_dir(),
        Err(error) => matches!(error.kind(), ErrorKind::NotFound | ErrorKind::NotADirectory),
    }
}

/// The listed tree with every file's digest. The listing is also what the
/// change poller compares the folder against.
struct Tree {
    files: Vec<Value>,
    skipped: Vec<Value>,
    listing: BTreeMap<String, (FileStat, String)>,
}

/// Read the tree at `root`. A path in `render_outputs` is a file a render
/// wrote, so it is left out of `files` and named in `skipped`. What the walk
/// or the read could not reach is named in `skipped` as well, with the reason:
/// a directory that cannot be read is listed as itself, and a file that cannot
/// be read is listed under its own path. Only a file that is confirmed gone is
/// left out without a word.
#[cfg(test)]
fn read_tree(root: &Path, render_outputs: &BTreeSet<String>) -> Result<Tree, WalkError> {
    read_tree_matching(root, render_outputs, None)
}

/// Like `read_tree`, but a file whose extension is not in `filter` is never
/// read, listed or named in `skipped`. Digests come from the digest cache, so
/// a file whose size and modification time are unchanged is not read again.
fn read_tree_matching(
    root: &Path,
    render_outputs: &BTreeSet<String>,
    filter: Option<&Extensions>,
) -> Result<Tree, WalkError> {
    let walked = walk_matching(root, filter)?;
    let mut tree = Tree {
        files: Vec::new(),
        skipped: walked
            .skipped
            .iter()
            .map(|(path, reason)| json!({"path": path, "reason": reason}))
            .collect(),
        listing: BTreeMap::new(),
    };
    for (path, stat) in walked.files {
        if render_outputs.contains(&path) {
            tree.skipped
                .push(json!({"path": path, "reason": "render output"}));
            continue;
        }
        match workspace::cached_digest(&root.join(&path), read_project_file) {
            Some((size, digest)) => {
                tree.files.push(json!({
                    "path": path,
                    "size": size,
                    "sha256": digest,
                    "mtime": stat.mtime_secs(),
                }));
                tree.listing.insert(path, (stat, digest));
            }
            // A file that vanished since the walk is left out, as if the walk
            // had never seen it. One that grew past the bound or cannot be
            // opened is still there, so it is named.
            None if confirmed_gone(root, &path, &walked.skipped) => {}
            None => tree
                .skipped
                .push(json!({"path": path, "reason": "unreadable"})),
        }
    }
    Ok(tree)
}

/// Read a project file whole, within the upload bound.
fn read_project_file(path: &Path) -> Option<Vec<u8>> {
    workspace::read_file_bounded_to(path, "project file", MAX_UPLOAD_BYTES).ok()
}

/// Resolve `relative` to a regular file inside `root`. The path must be safe,
/// name nothing hidden or ignored, and cross no symlink.
fn project_file(root: &Path, relative: &str) -> Result<PathBuf, (u16, &'static str)> {
    if !RelPath::is_valid(relative) {
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
    /// The digest the page holds for every file. A path in it is `changed`
    /// when it differs and `removed` when it vanishes; a path outside it is
    /// `added`.
    digests: BTreeMap<String, String>,
    /// Files a render wrote into the folder. They are never reported as added
    /// or changed. The poller refreshes this before every look.
    render_outputs: BTreeSet<String>,
    /// The result path of the render running now. A new file named for it is
    /// held back until the run ends and records it. The poller refreshes this
    /// before every look, together with `render_outputs`.
    running: Option<String>,
    /// The extensions the page asked the tree for. A file outside them is not
    /// walked into the scan, so it is never read and never reported. `None`
    /// means every file.
    extensions: Option<Extensions>,
}

impl Scan {
    /// Start from the listing the page last read. The first looks compare the
    /// folder with it, so a change made between that read and the first look
    /// is reported once its stat held still.
    fn from_listing(listing: &BTreeMap<String, (FileStat, String)>) -> Scan {
        let stats: BTreeMap<String, FileStat> = listing
            .iter()
            .map(|(path, (stat, _))| (path.clone(), *stat))
            .collect();
        Scan {
            settled: stats.clone(),
            seen: stats,
            digests: listing
                .iter()
                .map(|(path, (_, digest))| (path.clone(), digest.clone()))
                .collect(),
            render_outputs: BTreeSet::new(),
            running: None,
            extensions: None,
        }
    }

    /// Look only at files with these extensions, as the tree read did.
    fn with_extensions(mut self, extensions: Option<Extensions>) -> Scan {
        self.extensions = extensions;
        self
    }

    /// Whether `path` is a new file named for the result of the render running
    /// now. The command may be writing it, and `finish` records it when the
    /// run ends. A path the page holds is the editor's file and is never held
    /// back, however it is named.
    fn written_by_running_render(&self, path: &str) -> bool {
        let Some(result) = self.running.as_deref() else {
            return false;
        };
        !self.digests.contains_key(path) && named_for_result(result, path)
    }

    /// Look at the tree once. A file is reported when it held still across
    /// this look and the one before and differs from what was accepted
    /// earlier; a file that is gone is reported at once, but only when its
    /// absence is confirmed. A file a render wrote is skipped, and so is a new
    /// file named for the result of the render running now. A file the page
    /// holds that a render has recorded is reported removed once no render is
    /// running, since the render replaces it and the page must drop it. A tree
    /// that cannot be walked this time reports nothing and changes nothing, and
    /// neither does a part of it that cannot be read: its files stay as the
    /// page holds them.
    fn look(&mut self, root: &Path, record: &Path) -> Vec<Change> {
        let Ok(walked) = walk_matching(root, self.extensions.as_ref()) else {
            return Vec::new();
        };
        let (now, skipped) = (walked.files, walked.skipped);
        let mut changes = Vec::new();
        let mut recorded: Option<BTreeMap<String, String>> = None;
        // A held file that a render has recorded is the render's. The page is
        // told to drop it only when no render runs, since `finish` forgets it in
        // the written record before the running state ends, and a save that
        // omits it while the record still lists it would trash it.
        if self.running.is_none() {
            let replaced: Vec<String> = self
                .digests
                .keys()
                .filter(|path| self.render_outputs.contains(*path))
                .cloned()
                .collect();
            for path in replaced {
                self.settled.remove(&path);
                self.digests.remove(&path);
                changes.push(Change {
                    path,
                    kind: "removed",
                    sha256: None,
                    size: None,
                });
            }
        }
        for (path, stat) in &now {
            if self.render_outputs.contains(path) || self.written_by_running_render(path) {
                continue;
            }
            if self.seen.get(path) != Some(stat) || self.settled.get(path) == Some(stat) {
                continue;
            }
            let Some((size, digest)) =
                workspace::cached_digest(&root.join(path), read_project_file)
            else {
                // Still being written or just removed. The next look decides.
                continue;
            };
            let recorded = recorded
                .get_or_insert_with(|| workspace::recorded_digests(record).unwrap_or_default());
            // Either LibrePaper's own write, or content the page already holds.
            let own_write = recorded.get(path) == Some(&digest);
            let held = self.digests.get(path) == Some(&digest);
            if !own_write && !held {
                changes.push(Change {
                    path: path.clone(),
                    kind: if self.digests.contains_key(path) {
                        "changed"
                    } else {
                        "added"
                    },
                    sha256: Some(digest.clone()),
                    size: Some(size),
                });
            }
            self.settled.insert(path.clone(), *stat);
            self.digests.insert(path.clone(), digest);
        }
        let gone: Vec<String> = self
            .digests
            .keys()
            .filter(|path| !now.contains_key(*path) && confirmed_gone(root, path, &skipped))
            .cloned()
            .collect();
        for path in gone {
            self.settled.remove(&path);
            self.digests.remove(&path);
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
    /// A baseline from a tree read that the poller task has not taken yet.
    baseline: std::sync::Mutex<Option<Scan>>,
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
            baseline: std::sync::Mutex::new(None),
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

/// What renders have done to one project's folder, as the poller and the tree
/// listing see it. Both parts are read together, under one lock.
#[derive(Clone, Debug, Default)]
pub(super) struct RenderState {
    /// The files renders wrote. They are never reported as added or changed.
    pub outputs: BTreeSet<String>,
    /// The result path of the render running now, when its job names one.
    pub running: Option<String>,
}

impl RenderState {
    /// Record `paths` as outputs. Paths past `MAX_RENDER_OUTPUTS` are dropped
    /// rather than pushing out the ones held.
    fn record(&mut self, paths: impl IntoIterator<Item = String>) {
        for path in paths {
            if self.outputs.len() >= MAX_RENDER_OUTPUTS {
                break;
            }
            self.outputs.insert(path);
        }
    }
}

/// The render state of each project, by binding id.
type RenderStates = Arc<std::sync::Mutex<HashMap<String, RenderState>>>;

/// Marks a render as running for as long as it lives. Dropping it ends the
/// state however the run ended: finished, failed, cancelled or unwound. The
/// queue runs one job at a time, so a project never has two.
struct Running {
    renders: RenderStates,
    id: String,
}

impl Drop for Running {
    fn drop(&mut self) {
        let mut renders = self
            .renders
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(state) = renders.get_mut(&self.id) {
            state.running = None;
        }
    }
}

/// The polled projects of this service, by binding id.
pub(super) struct Registry {
    timing: Timing,
    pollers: Arc<std::sync::Mutex<HashMap<String, Arc<Shared>>>>,
    /// Kept beside the pollers, not on them: a poller stops when the page goes
    /// quiet, and the outputs are still not outside changes when the project
    /// is read again.
    renders: RenderStates,
}

impl Registry {
    pub(super) fn new(timing: Timing) -> Registry {
        Registry {
            timing,
            pollers: Arc::new(std::sync::Mutex::new(HashMap::new())),
            renders: Arc::new(std::sync::Mutex::new(HashMap::new())),
        }
    }

    pub(super) fn timing(&self) -> Timing {
        self.timing
    }

    /// Make `scan` the baseline for `binding`, as its tree was just read.
    /// Starts the poller task when none runs and otherwise replaces the
    /// baseline of the running one, keeping its queue and cursors. Answers the
    /// cursor at this moment, so events queued from the new baseline on come
    /// after it.
    fn set_baseline(&self, binding: &ProjectBinding, record: PathBuf, scan: Scan) -> u64 {
        let mut pollers = self
            .pollers
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(shared) = pollers.get(&binding.id) {
            // Under the map lock, so the task cannot decide to stop between
            // this lookup and the touch.
            shared.touch();
            *shared
                .baseline
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(scan);
            return shared.state().cursor;
        }
        let shared = Arc::new(Shared::new(self.timing.max_events));
        *shared
            .baseline
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(scan);
        pollers.insert(binding.id.clone(), shared.clone());
        let cursor = shared.state().cursor;
        tokio::spawn(run_poller(
            shared,
            self.pollers.clone(),
            binding.id.clone(),
            binding.root.clone(),
            record,
            self.renders.clone(),
            self.timing,
        ));
        cursor
    }

    /// Record `paths` as files a render wrote into the folder of project
    /// `id`, so its poller and its tree leave them out.
    pub(super) fn note_render_outputs(&self, id: &str, paths: impl IntoIterator<Item = String>) {
        self.renders
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .entry(id.to_string())
            .or_default()
            .record(paths);
    }

    /// A render starts in the folder of project `id`. Records the paths the
    /// command is expected to write and, when the job names a result, marks
    /// the render as running until the answer is dropped. Both happen under
    /// one lock, so the poller never sees one without the other.
    fn start_render(&self, id: &str, result: Option<String>, expected: Vec<String>) -> Running {
        {
            let mut renders = self
                .renders
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let state = renders.entry(id.to_string()).or_default();
            state.record(expected);
            state.running = result;
        }
        Running {
            renders: self.renders.clone(),
            id: id.to_string(),
        }
    }

    /// What renders have done to the folder of project `id`.
    pub(super) fn render_state(&self, id: &str) -> RenderState {
        render_state_of(&self.renders, id)
    }

    /// The paths renders wrote into the folder of project `id`.
    pub(super) fn render_outputs(&self, id: &str) -> BTreeSet<String> {
        self.render_state(id).outputs
    }

    /// The queue of a polled project, or `None` when no tree was read since
    /// its poller last stopped.
    fn get(&self, id: &str) -> Option<Arc<Shared>> {
        let pollers = self
            .pollers
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let shared = pollers.get(id)?;
        shared.touch();
        Some(shared.clone())
    }

    /// Stop polling `id` and drop its render outputs, as when its folder is
    /// forgotten.
    fn forget(&self, id: &str) {
        self.renders
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(id);
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

fn render_state_of(renders: &RenderStates, id: &str) -> RenderState {
    renders
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .get(id)
        .cloned()
        .unwrap_or_default()
}

async fn run_poller(
    shared: Arc<Shared>,
    pollers: Arc<std::sync::Mutex<HashMap<String, Arc<Shared>>>>,
    id: String,
    root: PathBuf,
    record: PathBuf,
    renders: RenderStates,
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
        let taken = shared
            .baseline
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take();
        if taken.is_some() {
            scan = taken;
        }
        let (root, record) = (root.clone(), record.clone());
        let state = render_state_of(&renders, &id);
        let looked = tokio::task::spawn_blocking(move || {
            let changes = match scan.as_mut() {
                Some(scan) => {
                    scan.render_outputs = state.outputs;
                    scan.running = state.running;
                    scan.look(&root, &record)
                }
                None => Vec::new(),
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

/* ------------------------------------------------------- render outputs */

/// A render running in a folder project, watched for the files it writes.
pub(super) struct RenderWatch {
    id: String,
    root: PathBuf,
    /// The paths the project lists. They are the editor's files, so the run
    /// is never credited with writing one.
    listed: HashSet<String>,
    /// The result path the command is expected to write. Other files are
    /// credited to the run only when they are named for it. `None` when the
    /// job names no tool record.
    result: Option<String>,
    /// Every path the command is expected to write: the result and, for a PDF,
    /// the SyncTeX files. They are recorded before it starts.
    expected: Vec<String>,
    /// The folder's files before the command ran, or `None` when it could not
    /// be listed, in which case only the expected outputs are recorded.
    before: Option<BTreeMap<String, FileStat>>,
    /// Keeps the project's running state until the watch is dropped.
    running: Running,
}

impl RenderWatch {
    /// Start watching `request`, which is about to run. `None` unless it
    /// names a folder project. The result and SyncTeX paths are recorded
    /// now, so the poller cannot report them while the command is still
    /// writing, and the project counts as running a render, so it holds back
    /// the other files named for the result until the run ends.
    pub(super) async fn begin(inner: &Inner, request: &JobRequest) -> Option<RenderWatch> {
        let binding = inner
            .bindings
            .get_project(&request.binding_id, &request.origin)
            .filter(|binding| binding.project == request.project)?;
        let expected = crate::local::render::expected_outputs(request);
        let result = expected.first().cloned();
        let running = inner
            .projects
            .start_render(&binding.id, result.clone(), expected.clone());
        let walked_root = binding.root.clone();
        let before = tokio::task::spawn_blocking(move || listed_files(&walked_root))
            .await
            .ok()
            .flatten();
        Some(RenderWatch {
            id: binding.id,
            root: binding.root,
            listed: request
                .manifest
                .iter()
                .map(|entry| entry.path.clone())
                .collect(),
            result,
            expected,
            before,
            running,
        })
    }

    /// The run is over, however it ended. Record every file it created or
    /// changed that is named for the result. Every path the run recorded, the
    /// expected outputs included, is then forgotten in the folder's written
    /// record, and nothing is trashed or moved. That is not done in `begin`: the
    /// job's own mirror runs after it and writes the page's copy of any such
    /// file the project lists, which puts it in the record again. The file is
    /// the render's now, and a save that omits it must not trash it.
    ///
    /// What the page holds is in the poller's scan, and what the mirror last
    /// wrote is in the record. A file can be in either or both. The record is
    /// forgotten here whatever the page holds. The poller reports a held file
    /// removed once the running state ends, and that is the last thing this
    /// does, whichever way it returns, so the page is never told to drop a file
    /// the record still lists. A poller that has stopped reports nothing, and
    /// the page's next `changes` request is answered with a reset, which reads
    /// the tree again without the file.
    pub(super) async fn finish(self, inner: &Inner) {
        let _running = self.running;
        let mut outputs: BTreeSet<String> = self.expected.into_iter().collect();
        if let (Some(before), Some(result)) = (self.before, self.result) {
            let root = self.root;
            let after = tokio::task::spawn_blocking(move || listed_files(&root)).await;
            if let Ok(Some(after)) = after {
                let written = written_since(&before, &after, &self.listed, &result);
                outputs.extend(written.iter().cloned());
                inner.projects.note_render_outputs(&self.id, written);
            }
        }
        // Past the bound a path is not recorded, and stays the person's.
        let recorded = inner.projects.render_outputs(&self.id);
        outputs.retain(|path| recorded.contains(path));
        if outputs.is_empty() {
            return;
        }
        let record = inner.bindings.written_record(&self.id);
        let forget = move || workspace::forget_recorded(&record, &outputs);
        let _ = tokio::task::spawn_blocking(forget).await;
    }
}

/// The files the poller would see in `root`, or `None` when the folder cannot
/// be walked.
fn listed_files(root: &Path) -> Option<BTreeMap<String, FileStat>> {
    walk(root).ok().map(|walked| walked.files)
}

/// The files in `after` that are new or differ from `before` and are named
/// for `result`, without the ones in `listed`. A command that rewrites a file
/// the project lists changed the editor's file, which the page has to hear
/// about. A file that is not named for the result may be another program's,
/// and recording it would hide every later change to it.
fn written_since(
    before: &BTreeMap<String, FileStat>,
    after: &BTreeMap<String, FileStat>,
    listed: &HashSet<String>,
    result: &str,
) -> Vec<String> {
    after
        .iter()
        .filter(|(path, stat)| before.get(*path) != Some(*stat))
        .map(|(path, _)| path)
        .filter(|path| !listed.contains(*path) && named_for_result(result, path))
        .cloned()
        .collect()
}

/// Whether `path` is named for the render result `result`: it sits in the
/// result's directory or below it, and the first name there is the result's
/// stem followed by `.`, `_` or `-`. For `chapters/main.pdf` that takes
/// `chapters/main.aux`, `chapters/main_files/libs/x.css` and
/// `chapters/main-figure0.pdf`, and leaves `chapters/refs.bib`, `main.aux`
/// and `data/x.csv` alone.
fn named_for_result(result: &str, path: &str) -> bool {
    let stem_path = crate::local::render::tool::stem_of(result);
    let (directory, stem) = match stem_path.rfind('/') {
        Some(slash) => stem_path.split_at(slash + 1),
        None => ("", stem_path),
    };
    if stem.is_empty() {
        return false;
    }
    let Some(rest) = path.strip_prefix(directory) else {
        return false;
    };
    let first = rest.split('/').next().unwrap_or_default();
    let Some(after) = first.strip_prefix(stem) else {
        return false;
    };
    matches!(after.chars().next(), Some('.' | '_' | '-'))
}

/* ------------------------------------------------------------- handlers */

fn folder_name(root: &Path) -> String {
    root.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// The project binding `id` of `origin`, with its folder present.
#[allow(clippy::result_large_err)]
pub(super) fn open_binding(
    inner: &Inner,
    origin: Option<&str>,
    id: &str,
) -> Result<ProjectBinding, Reply> {
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

/// Asks the person for a folder through the native chooser, which shows
/// `title`. The error is the answer to send: 409 while another chooser is
/// open, 204 when the person cancels.
#[allow(clippy::result_large_err)]
pub(super) async fn choose_root(inner: &Inner, title: &str) -> Result<PathBuf, Reply> {
    let Ok(_dialog) = inner.folder_dialog.try_lock() else {
        return Err(write_json(
            409,
            &json!({"error": "A folder chooser is already open on this computer."}),
        ));
    };
    match crate::local::folder::choose_directory_with_title(title).await {
        Ok(root) => Ok(root),
        Err(error) if error == crate::local::folder::CANCELLED => Err(plain_empty(204)),
        Err(error) => Err(write_json(400, &json!({"error": error}))),
    }
}

/// What opening a folder answers with.
pub(super) fn opened(binding: &ProjectBinding) -> Value {
    let name = folder_name(&binding.root);
    json!({
        "id": binding.id,
        "project": binding.project,
        "name": name,
        "root_name": name,
    })
}

pub(super) async fn handle_open(
    inner: &Arc<Inner>,
    headers: &HeaderMap,
    origin: Option<&str>,
) -> Reply {
    if let Err(response) = authenticate(inner, headers, origin) {
        return response;
    }
    let root = match choose_root(inner, "Open a folder as a LibrePaper project").await {
        Ok(root) => root,
        Err(response) => return response,
    };
    match inner
        .bindings
        .open_project(origin.unwrap_or_default(), &root)
    {
        Ok(binding) => write_json(200, &opened(&binding)),
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
    extensions: Option<&str>,
) -> Reply {
    if let Err(response) = authenticate(inner, headers, origin) {
        return response;
    }
    let binding = match open_binding(inner, origin, id) {
        Ok(binding) => binding,
        Err(response) => return response,
    };
    let extensions = extensions.and_then(parse_extensions);
    let _ = inner.bindings.touch_project(id, origin.unwrap_or_default());
    let record_path = inner.bindings.written_record(&binding.id);
    let root = binding.root.clone();
    let render_outputs = inner.projects.render_outputs(&binding.id);
    let walked_extensions = extensions.clone();
    let read = tokio::task::spawn_blocking(move || {
        let tree = read_tree_matching(&root, &render_outputs, walked_extensions.as_ref()).map_err(|error| match error {
            WalkError::TooMany => (
                413,
                format!(
                    "the folder holds more than {MAX_PROJECT_FILES} project files; open the folder that holds your project"
                ),
            ),
            WalkError::Io(error) => (500, format!("read the folder: {error}")),
        })?;
        Ok::<_, (u16, String)>(tree)
    })
    .await;
    match read {
        Ok(Ok(tree)) => {
            // The listing is what the page now holds. A file becomes managed
            // only by appearing in a `PUT folder` manifest, never by being
            // listed here, so the written record is not touched.
            let cursor = inner.projects.set_baseline(
                &binding,
                record_path,
                Scan::from_listing(&tree.listing).with_extensions(extensions),
            );
            write_json(
                200,
                &json!({"files": tree.files, "skipped": tree.skipped, "cursor": cursor}),
            )
        }
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
    // Nothing is polled until the page has read the tree, and a poller that
    // stopped for want of requests has forgotten its baseline. Either way the
    // page must read the tree again, which starts the poller.
    let Some(shared) = inner.projects.get(&binding.id) else {
        return write_json(200, &json!({"cursor": 0, "reset": true, "changes": []}));
    };
    write_json(
        200,
        &shared.wait(since, inner.projects.timing().long_poll).await,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    use crate::local::render::workspace::Unreadable;

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
    fn the_walk_refuses_more_files_than_the_cap_and_counts_only_matches() {
        let root = tempfile::tempdir().unwrap();
        for index in 0..3 {
            write(root.path(), &format!("d{}/f{index}.txt", index % 2), "");
        }
        for index in 0..10 {
            write(root.path(), &format!("other{index}.bin"), "");
        }
        assert_eq!(
            walk_limited(root.path(), None, 13, 100)
                .unwrap()
                .files
                .len(),
            13
        );
        assert!(matches!(
            walk_limited(root.path(), None, 12, 100),
            Err(WalkError::TooMany)
        ));
        // Unmatched files do not count toward the cap.
        let filter = names(&["txt"]);
        assert_eq!(
            walk_limited(root.path(), Some(&filter), 3, 100)
                .unwrap()
                .files
                .len(),
            3
        );
        assert!(matches!(
            walk_limited(root.path(), Some(&filter), 2, 100),
            Err(WalkError::TooMany)
        ));
    }

    #[test]
    fn the_walk_stops_after_too_many_directory_entries() {
        let root = tempfile::tempdir().unwrap();
        for index in 0..6 {
            write(root.path(), &format!("f{index}.bin"), "");
        }
        assert!(walk_limited(root.path(), None, 100, 6).is_ok());
        let filter = names(&["tex"]);
        assert!(matches!(
            walk_limited(root.path(), Some(&filter), 100, 5),
            Err(WalkError::TooMany)
        ));
    }

    #[test]
    fn the_extension_filter_matches_the_last_extension_ignoring_case() {
        let filter = names(&["tex", "md"]);
        let wanted = |name: &str| extension_matches(Some(&filter), name);
        assert!(wanted("main.tex"));
        assert!(wanted("MAIN.TeX"));
        assert!(wanted("notes.final.md"));
        assert!(!wanted("main.tex.bak"));
        assert!(!wanted("tex"));
        assert!(!wanted("main."));
        assert!(extension_matches(None, "tex"));
        assert_eq!(
            parse_extensions("tex, .MD,,pdf"),
            Some(names(&["tex", "md", "pdf"]))
        );
        assert_eq!(parse_extensions(" , "), None);
    }

    #[test]
    fn the_extension_filter_leaves_out_an_unmatched_file_without_reading_it() {
        let root = tempfile::tempdir().unwrap();
        write(root.path(), "main.tex", "hello");
        write(root.path(), "clip.mp4", "video");
        write(root.path(), "README", "no extension");
        let filter = names(&["tex"]);
        let locked = Unreadable::new(&root.path().join("clip.mp4"));
        let tree = read_tree_matching(root.path(), &BTreeSet::new(), Some(&filter)).unwrap();
        drop(locked);
        let listed: Vec<&Value> = tree.files.iter().map(|file| &file["path"]).collect();
        assert_eq!(listed, ["main.tex"]);
        assert!(tree.skipped.is_empty());
        assert!(!tree.listing.contains_key("clip.mp4"));
        // Without a filter every file is read.
        let everything = read_tree(root.path(), &BTreeSet::new()).unwrap();
        assert_eq!(everything.files.len(), 3);
    }

    #[test]
    fn the_poller_does_not_report_an_unmatched_file() {
        let (root, _state, record) = setup();
        write(root.path(), "main.tex", "one");
        write(root.path(), "clip.mp4", "video");
        let filter = names(&["tex"]);
        let tree = read_tree_matching(root.path(), &BTreeSet::new(), Some(&filter)).unwrap();
        let mut scan = Scan::from_listing(&tree.listing).with_extensions(Some(filter));
        write(root.path(), "clip.mp4", "a longer video");
        write(root.path(), "new.mp4", "another");
        std::fs::remove_file(root.path().join("clip.mp4")).unwrap();
        for _ in 0..3 {
            assert!(scan.look(root.path(), &record).is_empty());
        }
        // A matched file is still reported.
        write(root.path(), "late.tex", "new");
        let mut reported = Vec::new();
        for _ in 0..3 {
            reported.extend(scan.look(root.path(), &record));
        }
        assert_eq!(reported.len(), 1);
        assert_eq!(reported[0].path, "late.tex");
    }

    #[test]
    fn reading_the_tree_lists_files_and_leaves_the_written_record_alone() {
        let root = tempfile::tempdir().unwrap();
        write(root.path(), "main.tex", "hello");
        write(root.path(), "a/b.txt", "world");
        write(root.path(), ".git/config", "x");
        let tree = read_tree(root.path(), &BTreeSet::new()).unwrap();
        assert_eq!(tree.files.len(), 2);
        assert!(tree.files[0]["mtime"].is_u64());
        assert_eq!(
            tree.listing["main.tex"].1,
            librepaper_base::digest::Digest::of(b"hello").to_hex()
        );
    }

    #[test]
    fn a_listed_file_missing_from_a_manifest_survives_the_folder_mirror() {
        let root = tempfile::tempdir().unwrap();
        let state = tempfile::tempdir().unwrap();
        let staged = tempfile::tempdir().unwrap();
        write(root.path(), "main.tex", "hello");
        write(root.path(), "data.csv", "1,2,3");
        let record = state.path().join("record.json");
        // The page lists both files but keeps only main.tex in the project.
        let tree = read_tree(root.path(), &BTreeSet::new()).unwrap();
        assert!(tree.listing.contains_key("data.csv"));
        assert!(!record.exists());
        workspace::mirror_granted_folder(staged.path(), root.path(), &[], &record, false).unwrap();
        assert!(root.path().join("data.csv").exists());
        assert!(root.path().join("main.tex").exists());
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

    /// Read the tree, as the page does, and start a scan from its listing.
    fn seeded_scan(root: &Path, _record: &Path) -> Scan {
        Scan::from_listing(&read_tree(root, &BTreeSet::new()).unwrap().listing)
    }

    #[test]
    fn a_change_made_before_the_poller_starts_is_reported_once() {
        let (root, _state, record) = setup();
        write(root.path(), "main.tex", "one");
        write(root.path(), "gone.tex", "two");
        let mut scan = seeded_scan(root.path(), &record);
        // The poller starts later than the page's tree read.
        write(root.path(), "main.tex", "edited in between");
        write(root.path(), "late.tex", "new");
        std::fs::remove_file(root.path().join("gone.tex")).unwrap();
        // Added and changed files wait one poll for their stat to hold still.
        assert_eq!(scan.look(root.path(), &record).len(), 1);
        let mut first = scan.look(root.path(), &record);
        first.sort_by(|left, right| left.path.cmp(&right.path));
        let kinds: Vec<(&str, &str)> = first
            .iter()
            .map(|change| (change.path.as_str(), change.kind))
            .collect();
        assert_eq!(kinds, [("late.tex", "added"), ("main.tex", "changed")]);
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
            Some(librepaper_base::digest::Digest::of(b"changed text").to_hex().as_str())
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
            librepaper_base::digest::Digest::of(b"written by the editor").to_hex(),
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

    /* ------------------------------------------- removal needs confirmation */

    #[cfg(unix)]
    #[test]
    fn a_held_file_under_an_unreadable_directory_is_not_reported_removed() {
        let (root, _state, record) = setup();
        write(root.path(), "main.tex", "one");
        write(root.path(), "chapters/one.tex", "chapter");
        write(root.path(), "chapters/deep/two.tex", "deeper");
        let mut scan = seeded_scan(root.path(), &record);
        {
            let Some(_locked) = Unreadable::new(&root.path().join("chapters")) else {
                return;
            };
            for _ in 0..3 {
                assert!(scan.look(root.path(), &record).is_empty());
            }
            // The rest of the folder is still watched.
            std::fs::remove_file(root.path().join("main.tex")).unwrap();
            let changes = scan.look(root.path(), &record);
            assert_eq!(changes.len(), 1);
            assert_eq!(changes[0].path, "main.tex");
            assert_eq!(changes[0].kind, "removed");
        }
        // Readable again with nothing changed, so there is nothing to report.
        for _ in 0..3 {
            assert!(scan.look(root.path(), &record).is_empty());
        }
        // A file really deleted is reported.
        std::fs::remove_file(root.path().join("chapters/deep/two.tex")).unwrap();
        let changes = scan.look(root.path(), &record);
        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].path, "chapters/deep/two.tex");
        assert_eq!(changes[0].kind, "removed");
    }

    #[test]
    fn a_held_file_replaced_by_a_directory_is_reported_removed() {
        let (root, _state, record) = setup();
        write(root.path(), "main.tex", "one");
        write(root.path(), "notes", "a file the page holds");
        let mut scan = seeded_scan(root.path(), &record);
        std::fs::remove_file(root.path().join("notes")).unwrap();
        write(root.path(), "notes/a.tex", "inside");
        let changes = scan.look(root.path(), &record);
        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].path, "notes");
        assert_eq!(changes[0].kind, "removed");
    }

    #[test]
    fn a_skipped_entry_covers_itself_and_what_is_below_it() {
        assert!(covers("chapters", "chapters"));
        assert!(covers("chapters", "chapters/one.tex"));
        assert!(covers("a/b", "a/b/c/d.tex"));
        assert!(!covers("chapter", "chapters/one.tex"));
        assert!(!covers("chapters/one.tex", "chapters"));
        assert!(!covers("a/b", "a/c/b.tex"));
    }

    #[test]
    fn a_path_is_confirmed_gone_only_when_the_folder_says_so() {
        let (root, _state, _record) = setup();
        write(root.path(), "file.tex", "x");
        write(root.path(), "dir/inner.tex", "x");
        let skipped = [("locked".to_string(), "unreadable")];
        assert!(confirmed_gone(root.path(), "missing.tex", &skipped));
        assert!(confirmed_gone(root.path(), "file.tex/below.tex", &skipped));
        assert!(confirmed_gone(root.path(), "dir", &skipped));
        // Still a file, only not listed.
        assert!(!confirmed_gone(root.path(), "file.tex", &skipped));
        // The walk could not look there.
        assert!(!confirmed_gone(root.path(), "locked", &skipped));
        assert!(!confirmed_gone(root.path(), "locked/inner.tex", &skipped));
    }

    #[cfg(unix)]
    #[test]
    fn reading_the_tree_names_what_it_could_not_read_instead_of_dropping_it() {
        let root = tempfile::tempdir().unwrap();
        write(root.path(), "main.tex", "hello");
        write(root.path(), "locked/inner.tex", "inside");
        write(root.path(), "secret.tex", "private");
        let Some(_directory) = Unreadable::new(&root.path().join("locked")) else {
            return;
        };
        let Some(_file) = Unreadable::new(&root.path().join("secret.tex")) else {
            return;
        };
        let tree = read_tree(root.path(), &BTreeSet::new()).unwrap();
        let listed: Vec<&Value> = tree.files.iter().map(|file| &file["path"]).collect();
        assert_eq!(listed, ["main.tex"]);
        assert_eq!(tree.listing.len(), 1);
        for path in ["locked", "secret.tex"] {
            let named = json!({"path": path, "reason": "unreadable"});
            assert!(tree.skipped.contains(&named), "{path}");
        }
    }

    #[test]
    fn the_manifest_allowance_holds_every_file_a_folder_project_may_list() {
        let entries: Vec<Value> = (0..MAX_PROJECT_FILES)
            .map(|index| {
                json!({
                    "path": format!("{index:05}{}", "p".repeat(195)),
                    "sha256": "a".repeat(64),
                    "size": MAX_UPLOAD_BYTES,
                })
            })
            .collect();
        let text = json!({ "manifest": entries }).to_string();
        assert!(text.len() <= MAX_FOLDER_MANIFEST_BYTES, "{}", text.len());
        // The fixed bound for other JSON is far below it.
        assert!(text.len() > MAX_JSON_BYTES);
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
        let tree = read_tree(&root_path, &BTreeSet::new()).unwrap();
        // A change between the tree read and the poller's first scan.
        write(&root_path, "main.tex", "edited before the first scan");
        let _ = registry.set_baseline(&binding, record, Scan::from_listing(&tree.listing));
        let shared = registry.get(&binding.id).unwrap();
        let cursor = shared.wait(None, Duration::ZERO).await["cursor"]
            .as_u64()
            .unwrap();
        let answer = shared.wait(Some(cursor), timing.long_poll).await;
        assert_eq!(answer["changes"][0]["path"], "main.tex");
        assert_eq!(answer["changes"][0]["kind"], "changed");
        assert_eq!(answer["changes"].as_array().unwrap().len(), 1);
        // Nobody asks again, so the poller stops and leaves the registry.
        tokio::time::sleep(Duration::from_millis(800)).await;
        assert!(registry.pollers.lock().unwrap().get(&binding.id).is_none());
        assert!(shared.state().stopped);
    }

    #[tokio::test]
    async fn the_tree_route_lists_files_and_starts_a_poller_without_touching_the_record() {
        let state = tempfile::tempdir().unwrap();
        let root = tempfile::tempdir().unwrap();
        write(root.path(), "main.tex", "hello");
        write(root.path(), ".git/config", "x");
        let inner = test_support::inner(state.path(), state.path()).await;
        let origin = "https://paper.example";
        let binding = inner.bindings.open_project(origin, root.path()).unwrap();
        let headers = test_support::bearer(&inner);

        let reply = handle_tree(&inner, &headers, Some(origin), &binding.id, None).await;
        assert_eq!(reply.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(reply.into_body(), 1 << 20)
            .await
            .unwrap();
        let body: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(body["files"][0]["path"], "main.tex");
        assert_eq!(body["files"][0]["size"], 5);
        assert_eq!(body["skipped"][0]["path"], ".git");
        assert!(!inner.bindings.written_record(&binding.id).exists());
        assert!(inner.projects.get(&binding.id).is_some());

        // Another origin cannot read it.
        let foreign = handle_tree(
            &inner,
            &headers,
            Some("https://other.example"),
            &binding.id,
            None,
        )
        .await;
        assert_eq!(foreign.status(), StatusCode::NOT_FOUND);
        let file = handle_file(&inner, &headers, Some(origin), &binding.id, "main.tex").await;
        let bytes = axum::body::to_bytes(file.into_body(), 1 << 20)
            .await
            .unwrap();
        assert_eq!(&bytes[..], b"hello");
        let hidden = handle_file(&inner, &headers, Some(origin), &binding.id, ".git/config").await;
        assert_eq!(hidden.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn changes_before_any_tree_read_ask_the_page_to_reset() {
        let state = tempfile::tempdir().unwrap();
        let root = tempfile::tempdir().unwrap();
        let inner = test_support::inner(state.path(), state.path()).await;
        let origin = "https://paper.example";
        let binding = inner.bindings.open_project(origin, root.path()).unwrap();
        let headers = test_support::bearer(&inner);
        let request = Request::get("/changes?since=5")
            .body(Body::empty())
            .unwrap();
        let reply = handle_changes(&inner, &headers, Some(origin), &binding.id, request).await;
        assert_eq!(reply.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(reply.into_body(), 1 << 20)
            .await
            .unwrap();
        let body: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(body["reset"], true);
        assert_eq!(body["changes"], json!([]));
    }

    #[tokio::test]
    async fn changes_since_the_tree_cursor_report_a_change_made_after_the_tree_read() {
        let state = tempfile::tempdir().unwrap();
        let root = tempfile::tempdir().unwrap();
        let root_path = std::fs::canonicalize(root.path()).unwrap();
        write(&root_path, "main.tex", "one");
        let inner = test_support::inner(state.path(), state.path()).await;
        let origin = "https://paper.example";
        let binding = inner.bindings.open_project(origin, &root_path).unwrap();
        let headers = test_support::bearer(&inner);

        let reply = handle_tree(&inner, &headers, Some(origin), &binding.id, None).await;
        let bytes = axum::body::to_bytes(reply.into_body(), 1 << 20)
            .await
            .unwrap();
        let tree: Value = serde_json::from_slice(&bytes).unwrap();
        let cursor = tree["cursor"].as_u64().unwrap();

        write(&root_path, "main.tex", "changed after the read");
        let request = Request::get(format!("/changes?since={cursor}"))
            .body(Body::empty())
            .unwrap();
        let reply = tokio::time::timeout(
            Duration::from_secs(10),
            handle_changes(&inner, &headers, Some(origin), &binding.id, request),
        )
        .await
        .unwrap();
        let bytes = axum::body::to_bytes(reply.into_body(), 1 << 20)
            .await
            .unwrap();
        let body: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(body["changes"][0]["path"], "main.tex");
        assert_eq!(body["changes"][0]["kind"], "changed");
    }

    /* ------------------------------------------------------ render outputs */

    const ORIGIN: &str = "https://paper.example";

    fn names(items: &[&str]) -> BTreeSet<String> {
        items.iter().map(|item| (*item).to_string()).collect()
    }

    #[test]
    fn a_render_output_is_not_reported_but_an_unrelated_file_is() {
        let (root, _state, record) = setup();
        write(root.path(), "main.tex", "one");
        let mut scan = seeded_scan(root.path(), &record);
        scan.render_outputs = names(&["main.pdf", "main.synctex.gz"]);

        write(root.path(), "main.pdf", "%PDF first run");
        write(root.path(), "main.synctex.gz", "sync");
        write(root.path(), "notes.txt", "written at the same time");
        assert!(scan.look(root.path(), &record).is_empty());
        let changes = scan.look(root.path(), &record);
        let reported: Vec<(&str, &str)> = changes
            .iter()
            .map(|change| (change.path.as_str(), change.kind))
            .collect();
        assert_eq!(reported, [("notes.txt", "added")]);

        // A later run writes the PDF again with other bytes. Still nothing.
        write(root.path(), "main.pdf", "%PDF second run, different bytes");
        for _ in 0..3 {
            assert!(scan.look(root.path(), &record).is_empty());
        }
    }

    #[test]
    fn a_held_file_a_render_records_is_reported_removed_once_the_run_ends() {
        let (root, _state, record) = setup();
        write(root.path(), "main.tex", "one");
        write(root.path(), "main.pdf", "%PDF the page imported");
        let mut scan = seeded_scan(root.path(), &record);
        scan.running = Some("main.pdf".into());
        scan.render_outputs = names(&["main.pdf", "main.synctex.gz"]);
        write(root.path(), "main.pdf", "%PDF the render wrote");
        // The page keeps its copy while the command runs.
        for _ in 0..3 {
            assert!(scan.look(root.path(), &record).is_empty());
        }

        scan.running = None;
        let changes = scan.look(root.path(), &record);
        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].path, "main.pdf");
        assert_eq!(changes[0].kind, "removed");
        for _ in 0..3 {
            assert!(scan.look(root.path(), &record).is_empty());
        }

        // The page no longer holds it, so a later run or a delete is the
        // render's business and nothing is reported.
        write(root.path(), "main.pdf", "%PDF the next run wrote");
        for _ in 0..3 {
            assert!(scan.look(root.path(), &record).is_empty());
        }
        std::fs::remove_file(root.path().join("main.pdf")).unwrap();
        assert!(scan.look(root.path(), &record).is_empty());
    }

    #[test]
    fn the_tree_leaves_render_outputs_out_and_names_them_skipped() {
        let root = tempfile::tempdir().unwrap();
        write(root.path(), "main.tex", "hello");
        write(root.path(), "main.pdf", "%PDF");
        let tree = read_tree(root.path(), &names(&["main.pdf"])).unwrap();
        assert_eq!(tree.files.len(), 1);
        assert_eq!(tree.files[0]["path"], "main.tex");
        assert!(!tree.listing.contains_key("main.pdf"));
        let skipped = json!({"path": "main.pdf", "reason": "render output"});
        assert!(tree.skipped.contains(&skipped));
    }

    #[test]
    fn a_file_is_named_for_the_result_by_its_directory_and_first_name() {
        for path in [
            "main.pdf",
            "main.aux",
            "main.synctex.gz",
            "main_files/libs/x.css",
            "main-figure0.pdf",
        ] {
            assert!(named_for_result("main.pdf", path), "{path}");
        }
        for path in [
            "refs.bib",
            "data/x.csv",
            "chapters/one.tex",
            "chapters/main.aux",
            "mainstream.png",
            "main",
            "main/x.png",
            "sub/main.aux",
        ] {
            assert!(!named_for_result("main.pdf", path), "{path}");
        }
        // A result in a folder claims that folder and below, and nothing above.
        for path in [
            "chapters/main.aux",
            "chapters/main_files/a.css",
            "chapters/main-1.pdf",
        ] {
            assert!(named_for_result("chapters/main.pdf", path), "{path}");
        }
        for path in [
            "main.aux",
            "chapters/refs.bib",
            "chapters/sub/main.aux",
            "other/main.aux",
        ] {
            assert!(!named_for_result("chapters/main.pdf", path), "{path}");
        }
    }

    #[test]
    fn the_files_a_run_wrote_are_the_new_and_changed_ones_named_for_its_result() {
        let stat = |size| FileStat { size, mtime_ns: 5 };
        let before: BTreeMap<String, FileStat> = [
            ("main.pdf".to_string(), stat(2)),
            ("main.tex".to_string(), stat(1)),
            ("same.png".to_string(), stat(3)),
        ]
        .into();
        let after: BTreeMap<String, FileStat> = [
            ("chapters/one.tex".to_string(), stat(1)),
            ("data/x.csv".to_string(), stat(1)),
            ("main-figure0.pdf".to_string(), stat(1)),
            ("main.aux".to_string(), stat(1)),
            ("main.pdf".to_string(), stat(4)),
            ("main.tex".to_string(), stat(9)),
            ("main_files/libs/x.css".to_string(), stat(1)),
            ("refs.bib".to_string(), stat(1)),
            ("same.png".to_string(), stat(3)),
        ]
        .into();
        let listed: HashSet<String> = ["main.tex".to_string()].into();
        assert_eq!(
            written_since(&before, &after, &listed, "main.pdf"),
            [
                "main-figure0.pdf",
                "main.aux",
                "main.pdf",
                "main_files/libs/x.css",
            ]
        );
    }

    #[test]
    fn the_render_output_record_is_bounded_and_keeps_what_it_holds() {
        let registry = Registry::new(Timing::default());
        registry.note_render_outputs("project", ["main.pdf".to_string()]);
        registry.note_render_outputs(
            "project",
            (0..MAX_RENDER_OUTPUTS + 10).map(|index| format!("figure-{index}.png")),
        );
        let held = registry.render_outputs("project");
        assert_eq!(held.len(), MAX_RENDER_OUTPUTS);
        assert!(held.contains("main.pdf"));
        assert!(registry.render_outputs("another").is_empty());
    }

    #[tokio::test]
    async fn the_poller_task_leaves_render_outputs_out_of_its_changes() {
        let (root, state, _record) = setup();
        let root_path = std::fs::canonicalize(root.path()).unwrap();
        write(&root_path, "main.tex", "one");
        let timing = Timing {
            interval: Duration::from_millis(20),
            long_poll: Duration::from_secs(5),
            idle: Duration::from_secs(30),
            max_events: 100,
        };
        let inner = test_support::inner(state.path(), state.path()).await;
        let binding = inner.bindings.open_project(ORIGIN, &root_path).unwrap();
        let registry = Registry::new(timing);
        let record = inner.bindings.written_record(&binding.id);
        let tree = read_tree(&root_path, &BTreeSet::new()).unwrap();
        let _ = registry.set_baseline(&binding, record, Scan::from_listing(&tree.listing));
        let shared = registry.get(&binding.id).unwrap();
        let cursor = shared.wait(None, Duration::ZERO).await["cursor"]
            .as_u64()
            .unwrap();

        let outputs = ["main.pdf", "main.synctex.gz"].map(String::from);
        registry.note_render_outputs(&binding.id, outputs);
        write(&root_path, "main.pdf", "%PDF");
        write(&root_path, "main.synctex.gz", "sync");
        write(&root_path, "notes.txt", "notes");
        let answer = shared.wait(Some(cursor), timing.long_poll).await;
        let paths: Vec<&Value> = answer["changes"]
            .as_array()
            .unwrap()
            .iter()
            .map(|change| &change["path"])
            .collect();
        assert_eq!(paths, ["notes.txt"]);
        // The outputs settled in the same looks, so they are not still coming.
        let cursor = answer["cursor"].as_u64().unwrap();
        let next = shared.wait(Some(cursor), Duration::from_millis(300)).await;
        assert_eq!(next["changes"], json!([]));
    }

    /// An `Inner` whose pollers look a few times a second and whose long
    /// polls end after a second and a half. A file a run created is recorded
    /// when the run ends, so the looks must be slower than the test's write
    /// followed by `finish`.
    async fn quick_inner(state: &Path) -> Arc<Inner> {
        timed_inner(
            state,
            Timing {
                interval: Duration::from_millis(250),
                long_poll: Duration::from_millis(1500),
                idle: Duration::from_secs(30),
                max_events: 100,
            },
        )
        .await
    }

    async fn timed_inner(state: &Path, timing: Timing) -> Arc<Inner> {
        let mut inner = test_support::inner(state, state).await;
        Arc::get_mut(&mut inner).expect("not shared yet").projects = Registry::new(timing);
        inner
    }

    fn render_request(binding: &ProjectBinding, entrypoint: &str) -> JobRequest {
        JobRequest {
            protocol: 3,
            project: binding.project.clone(),
            origin: ORIGIN.into(),
            snapshot: "snapshot".into(),
            generation: 1,
            format: "latex".into(),
            output: "pdf".into(),
            entrypoint: entrypoint.into(),
            binding_id: binding.id.clone(),
            manifest: vec![ManifestEntry {
                path: entrypoint.into(),
                sha256: librepaper_base::digest::Digest::of(b"hello").to_hex(),
                size: 5,
            }],
            options: Default::default(),
        }
    }

    async fn body_of(reply: Reply) -> Value {
        let bytes = axum::body::to_bytes(reply.into_body(), 1 << 20)
            .await
            .unwrap();
        serde_json::from_slice(&bytes).unwrap()
    }

    #[tokio::test]
    async fn a_render_in_a_folder_project_is_not_an_outside_change_and_not_in_the_tree() {
        let state = tempfile::tempdir().unwrap();
        let root = tempfile::tempdir().unwrap();
        let root_path = std::fs::canonicalize(root.path()).unwrap();
        write(&root_path, "main.tex", "hello");
        let inner = quick_inner(state.path()).await;
        let binding = inner.bindings.open_project(ORIGIN, &root_path).unwrap();
        let headers = test_support::bearer(&inner);

        let tree =
            body_of(handle_tree(&inner, &headers, Some(ORIGIN), &binding.id, None).await).await;
        let cursor = tree["cursor"].as_u64().unwrap();

        // The render: the expected outputs and files the run created, written
        // between `begin` and `finish`. Another program saves `refs.bib` in
        // the same window.
        let render = RenderWatch::begin(&inner, &render_request(&binding, "main.tex"))
            .await
            .expect("a folder project is watched");
        // The result and SyncTeX paths are recorded before the command runs.
        let held = inner.projects.render_outputs(&binding.id);
        assert!(held.is_superset(&names(&["main.pdf", "main.synctex.gz", "main.synctex"])));
        write(&root_path, "main.pdf", "%PDF");
        write(&root_path, "main.synctex.gz", "sync");
        write(&root_path, "main.aux", "aux");
        write(&root_path, "main_files/libs/x.css", "css");
        write(&root_path, "refs.bib", "@book{key}");
        render.finish(&inner).await;

        let changes = |since: u64| {
            let request = Request::get(format!("/changes?since={since}"))
                .body(Body::empty())
                .unwrap();
            handle_changes(&inner, &headers, Some(ORIGIN), &binding.id, request)
        };
        let mut answer = body_of(changes(cursor).await).await;
        for _ in 0..10 {
            if !answer["changes"].as_array().unwrap().is_empty() {
                break;
            }
            answer = body_of(changes(cursor).await).await;
        }
        assert_eq!(answer["changes"].as_array().unwrap().len(), 1);
        assert_eq!(answer["changes"][0]["path"], "refs.bib");
        assert_eq!(answer["changes"][0]["kind"], "added");
        // The outputs are not still on their way.
        let next = body_of(changes(answer["cursor"].as_u64().unwrap()).await).await;
        assert_eq!(next["changes"], json!([]));

        // A folder opened after the render does not list its own outputs.
        let tree =
            body_of(handle_tree(&inner, &headers, Some(ORIGIN), &binding.id, None).await).await;
        let listed: Vec<&Value> = tree["files"]
            .as_array()
            .unwrap()
            .iter()
            .map(|file| &file["path"])
            .collect();
        assert_eq!(listed, ["main.tex", "refs.bib"]);
        let skipped: Vec<&Value> = tree["skipped"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|entry| entry["reason"] == "render output")
            .map(|entry| &entry["path"])
            .collect();
        assert_eq!(
            skipped,
            [
                "main.aux",
                "main.pdf",
                "main.synctex.gz",
                "main_files/libs/x.css",
            ]
        );
    }

    #[tokio::test]
    async fn only_a_folder_project_render_is_watched() {
        let state = tempfile::tempdir().unwrap();
        let root = tempfile::tempdir().unwrap();
        let inner = quick_inner(state.path()).await;
        let binding = inner.bindings.open_project(ORIGIN, root.path()).unwrap();
        let mut hosted = render_request(&binding, "main.tex");
        hosted.binding_id = "hosted".into();
        assert!(RenderWatch::begin(&inner, &hosted).await.is_none());
        let mut other = render_request(&binding, "main.tex");
        other.project = "another".into();
        assert!(RenderWatch::begin(&inner, &other).await.is_none());
        assert!(inner.projects.render_outputs(&binding.id).is_empty());
    }

    #[tokio::test]
    async fn forgetting_a_project_clears_its_render_outputs() {
        let state = tempfile::tempdir().unwrap();
        let root = tempfile::tempdir().unwrap();
        let inner = quick_inner(state.path()).await;
        let binding = inner.bindings.open_project(ORIGIN, root.path()).unwrap();
        let headers = test_support::bearer(&inner);
        let render = RenderWatch::begin(&inner, &render_request(&binding, "main.tex"))
            .await
            .unwrap();
        render.finish(&inner).await;
        let held = inner.projects.render_outputs(&binding.id);
        assert!(held.contains("main.pdf"));

        let reply = handle_forget(&inner, &headers, Some(ORIGIN), &binding.id).await;
        assert_eq!(reply.status(), StatusCode::OK);
        assert!(inner.projects.render_outputs(&binding.id).is_empty());
    }

    #[test]
    fn during_a_render_a_new_file_named_for_its_result_is_held_back_but_inputs_are_not() {
        let (root, _state, record) = setup();
        write(root.path(), "main.tex", "one");
        write(root.path(), "chapters/one.tex", "first");
        let mut scan = seeded_scan(root.path(), &record);
        scan.running = Some("main.pdf".into());

        // The command writes a figure early and keeps running. Meanwhile
        // another program edits a chapter and the source the page holds.
        write(root.path(), "main-figure.png", "half");
        write(root.path(), "chapters/one.tex", "edited outside");
        write(root.path(), "main.tex", "edited too");
        assert!(scan.look(root.path(), &record).is_empty());
        write(root.path(), "main-figure.png", "the whole figure");
        let mut changes = scan.look(root.path(), &record);
        changes.sort_by(|left, right| left.path.cmp(&right.path));
        let reported: Vec<(&str, &str)> = changes
            .iter()
            .map(|change| (change.path.as_str(), change.kind))
            .collect();
        assert_eq!(
            reported,
            [("chapters/one.tex", "changed"), ("main.tex", "changed")]
        );
        for _ in 0..3 {
            assert!(scan.look(root.path(), &record).is_empty());
        }

        // The run ends and records the figure. It stays unreported.
        scan.render_outputs = names(&["main-figure.png"]);
        scan.running = None;
        for _ in 0..3 {
            assert!(scan.look(root.path(), &record).is_empty());
        }
    }

    #[test]
    fn during_a_render_a_new_file_not_named_for_its_result_is_still_reported() {
        let (root, _state, record) = setup();
        write(root.path(), "main.tex", "one");
        let mut scan = seeded_scan(root.path(), &record);
        scan.running = Some("main.pdf".into());
        write(root.path(), "refs.bib", "@book{key}");
        assert!(scan.look(root.path(), &record).is_empty());
        let changes = scan.look(root.path(), &record);
        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].path, "refs.bib");
        assert_eq!(changes[0].kind, "added");
    }

    #[tokio::test]
    async fn while_a_render_runs_the_poller_holds_back_its_files_and_still_reports_inputs() {
        let state = tempfile::tempdir().unwrap();
        let root = tempfile::tempdir().unwrap();
        let root_path = std::fs::canonicalize(root.path()).unwrap();
        write(&root_path, "main.tex", "hello");
        write(&root_path, "chapters/one.tex", "first");
        let inner = quick_inner(state.path()).await;
        let binding = inner.bindings.open_project(ORIGIN, &root_path).unwrap();
        let headers = test_support::bearer(&inner);
        let tree =
            body_of(handle_tree(&inner, &headers, Some(ORIGIN), &binding.id, None).await).await;
        let cursor = tree["cursor"].as_u64().unwrap();

        let render = RenderWatch::begin(&inner, &render_request(&binding, "main.tex"))
            .await
            .expect("a folder project is watched");
        let running = inner.projects.render_state(&binding.id).running;
        assert_eq!(running.as_deref(), Some("main.pdf"));
        // The command writes a figure early and keeps running. The person
        // edits a chapter in the meantime.
        write(&root_path, "main-figure.png", "png bytes");
        write(&root_path, "chapters/one.tex", "edited by the person");

        let changes = |since: u64| {
            let request = Request::get(format!("/changes?since={since}"))
                .body(Body::empty())
                .unwrap();
            handle_changes(&inner, &headers, Some(ORIGIN), &binding.id, request)
        };
        let mut answer = body_of(changes(cursor).await).await;
        for _ in 0..10 {
            if !answer["changes"].as_array().unwrap().is_empty() {
                break;
            }
            answer = body_of(changes(cursor).await).await;
        }
        let reported: Vec<(&str, &str)> = answer["changes"]
            .as_array()
            .unwrap()
            .iter()
            .map(|change| {
                (
                    change["path"].as_str().unwrap(),
                    change["kind"].as_str().unwrap(),
                )
            })
            .collect();
        assert_eq!(reported, [("chapters/one.tex", "changed")]);

        // The run ends. The figure is recorded and stays unreported.
        render.finish(&inner).await;
        let after = inner.projects.render_state(&binding.id);
        assert_eq!(after.running, None);
        assert!(after.outputs.contains("main-figure.png"));
        let next = body_of(changes(answer["cursor"].as_u64().unwrap()).await).await;
        assert_eq!(next["changes"], json!([]));
    }

    #[tokio::test]
    async fn a_watch_dropped_without_finishing_ends_the_running_state() {
        let state = tempfile::tempdir().unwrap();
        let root = tempfile::tempdir().unwrap();
        let inner = quick_inner(state.path()).await;
        let binding = inner.bindings.open_project(ORIGIN, root.path()).unwrap();
        let render = RenderWatch::begin(&inner, &render_request(&binding, "main.tex"))
            .await
            .unwrap();
        assert!(inner.projects.render_state(&binding.id).running.is_some());
        drop(render);
        let after = inner.projects.render_state(&binding.id);
        assert_eq!(after.running, None);
        // What the command was expected to write stays recorded.
        assert!(after.outputs.contains("main.pdf"));
    }

    fn digest(text: &str) -> String {
        librepaper_base::digest::Digest::of(text.as_bytes()).to_hex()
    }

    /// A save of the page that lists only `main.tex`, with nothing to upload.
    fn save_of_main_tex_only(root: &Path, record: &Path) -> workspace::MirrorReport {
        let staged = tempfile::tempdir().unwrap();
        let manifest = [ManifestEntry {
            path: "main.tex".into(),
            sha256: digest("hello"),
            size: 5,
        }];
        workspace::mirror_granted_folder(staged.path(), root, &manifest, record, false).unwrap()
    }

    #[tokio::test]
    async fn a_render_replacing_an_imported_file_drops_it_from_the_page_and_the_record() {
        let state = tempfile::tempdir().unwrap();
        let root = tempfile::tempdir().unwrap();
        let root_path = std::fs::canonicalize(root.path()).unwrap();
        write(&root_path, "main.tex", "hello");
        write(&root_path, "main.pdf", "%PDF built outside");
        let inner = quick_inner(state.path()).await;
        let binding = inner.bindings.open_project(ORIGIN, &root_path).unwrap();
        let headers = test_support::bearer(&inner);

        // The page opens the folder, imports both files, and saves them.
        let tree =
            body_of(handle_tree(&inner, &headers, Some(ORIGIN), &binding.id, None).await).await;
        let cursor = tree["cursor"].as_u64().unwrap();
        let listed: Vec<&Value> = tree["files"]
            .as_array()
            .unwrap()
            .iter()
            .map(|file| &file["path"])
            .collect();
        assert_eq!(listed, ["main.pdf", "main.tex"]);
        let record = inner.bindings.written_record(&binding.id);
        let saved = BTreeMap::from([
            ("main.pdf".to_string(), digest("%PDF built outside")),
            ("main.tex".to_string(), digest("hello")),
        ]);
        workspace::record_listed_digests(&record, &root_path, &saved).unwrap();

        let render = RenderWatch::begin(&inner, &render_request(&binding, "main.tex"))
            .await
            .expect("a folder project is watched");
        write(&root_path, "main.pdf", "%PDF the render wrote");
        render.finish(&inner).await;

        let changes = |since: u64| {
            let request = Request::get(format!("/changes?since={since}"))
                .body(Body::empty())
                .unwrap();
            handle_changes(&inner, &headers, Some(ORIGIN), &binding.id, request)
        };
        let mut answer = body_of(changes(cursor).await).await;
        for _ in 0..10 {
            if !answer["changes"].as_array().unwrap().is_empty() {
                break;
            }
            answer = body_of(changes(cursor).await).await;
        }
        assert_eq!(
            answer["changes"],
            json!([{"path": "main.pdf", "kind": "removed"}])
        );
        // The page is told once.
        let next = body_of(changes(answer["cursor"].as_u64().unwrap()).await).await;
        assert_eq!(next["changes"], json!([]));

        // The folder no longer manages the file, and the others stay recorded.
        let recorded = workspace::recorded_digests(&record).unwrap();
        assert!(!recorded.contains_key("main.pdf"));
        assert!(recorded.contains_key("main.tex"));
        // The page dropped it, so its next save omits it and trashes nothing.
        let report = save_of_main_tex_only(&root_path, &record);
        assert!(report.trashed.is_empty());
        assert_eq!(
            std::fs::read(root_path.join("main.pdf")).unwrap(),
            b"%PDF the render wrote"
        );
    }

    #[tokio::test]
    async fn a_page_whose_poller_stopped_reads_the_tree_again_and_the_record_is_forgotten() {
        let state = tempfile::tempdir().unwrap();
        let root = tempfile::tempdir().unwrap();
        let root_path = std::fs::canonicalize(root.path()).unwrap();
        write(&root_path, "main.tex", "hello");
        write(&root_path, "main.pdf", "%PDF built outside");
        let inner = timed_inner(
            state.path(),
            Timing {
                interval: Duration::from_millis(40),
                long_poll: Duration::from_millis(1500),
                idle: Duration::from_millis(200),
                max_events: 100,
            },
        )
        .await;
        let binding = inner.bindings.open_project(ORIGIN, &root_path).unwrap();
        let headers = test_support::bearer(&inner);
        let tree =
            body_of(handle_tree(&inner, &headers, Some(ORIGIN), &binding.id, None).await).await;
        let cursor = tree["cursor"].as_u64().unwrap();
        let record = inner.bindings.written_record(&binding.id);
        let saved = BTreeMap::from([
            ("main.pdf".to_string(), digest("%PDF built outside")),
            ("main.tex".to_string(), digest("hello")),
        ]);
        workspace::record_listed_digests(&record, &root_path, &saved).unwrap();

        // Nobody asks for changes, so the poller stops and forgets its scan.
        tokio::time::timeout(Duration::from_secs(5), async {
            while inner
                .projects
                .pollers
                .lock()
                .unwrap()
                .contains_key(&binding.id)
            {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .expect("the poller stops");

        let render = RenderWatch::begin(&inner, &render_request(&binding, "main.tex"))
            .await
            .expect("a folder project is watched");
        write(&root_path, "main.pdf", "%PDF the render wrote");
        render.finish(&inner).await;
        assert!(!workspace::recorded_digests(&record)
            .unwrap()
            .contains_key("main.pdf"));

        // With no poller to report it, the page is told to read the tree
        // again, and the tree no longer lists the file.
        let request = Request::get(format!("/changes?since={cursor}"))
            .body(Body::empty())
            .unwrap();
        let reply = handle_changes(&inner, &headers, Some(ORIGIN), &binding.id, request).await;
        let answer = body_of(reply).await;
        assert_eq!(answer["reset"], true);
        assert_eq!(answer["changes"], json!([]));
        let tree =
            body_of(handle_tree(&inner, &headers, Some(ORIGIN), &binding.id, None).await).await;
        let listed: Vec<&Value> = tree["files"]
            .as_array()
            .unwrap()
            .iter()
            .map(|file| &file["path"])
            .collect();
        assert_eq!(listed, ["main.tex"]);
        let skipped = json!({"path": "main.pdf", "reason": "render output"});
        assert!(tree["skipped"].as_array().unwrap().contains(&skipped));

        let report = save_of_main_tex_only(&root_path, &record);
        assert!(report.trashed.is_empty());
        assert_eq!(
            std::fs::read(root_path.join("main.pdf")).unwrap(),
            b"%PDF the render wrote"
        );
    }
}
