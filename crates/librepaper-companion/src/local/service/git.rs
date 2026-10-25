//! Git routes on a folder project: the person's own Git, run in the folder.
//!
//! This file authenticates, finds the folder, refuses what must not run, and
//! shapes the answers. The runtime in `crate::local::git` finds Git, makes it
//! non-interactive and bounds it. A folder is a repository when it holds a
//! `.git`; every route but `git/init` needs one, and answers 409 with the
//! reason `no-repository` otherwise.
//!
//! LibrePaper never merges text, never holds a forge credential and never
//! forces anything. A pull is `git pull --no-rebase`: a conflicting one leaves
//! the merge in progress with markers in the files, and the next commit
//! finishes it. A push never pulls; a branch that fell behind is a 409 with
//! the reason `behind`. A failure of the network, a missing credential or a
//! missing repository is a 502 with the reason `network`, `auth` or
//! `not-found` and the redacted words of Git as `detail`. Any other refusal by
//! Git is a 422 with its words as `error`.
//!
//! The change poller needs nothing from here. What Git writes into the folder
//! is an outside change like any other, so a pull or a switch reaches the
//! editor through the poll.

use super::*;

use std::collections::BTreeSet;

use crate::local::bindings::ProjectBinding;
use crate::local::git::status::{self, Status};
use crate::local::git::{self, remote, validate, Limit, Output, RunError};

/// The message `sync` commits with when the request names none.
const SYNC_MESSAGE: &str = "Edits from LibrePaper";
/// The longest commit message taken.
const MAX_MESSAGE_BYTES: usize = 16 * 1024;
/// The largest side of a diff, in bytes.
const MAX_DIFF_BYTES: usize = 8 * 1024 * 1024;
/// How much of a file is looked at for a NUL byte, as Git does.
const BINARY_PROBE_BYTES: usize = 8000;

/// A refusal: the status and the JSON body to answer with.
type Failure = (u16, Value);

fn refuse(status: u16, error: &str) -> Failure {
    (status, json!({"error": error}))
}

fn refuse_because(status: u16, reason: &str, error: &str) -> Failure {
    (status, json!({"reason": reason, "error": error}))
}

fn answer(result: Result<Value, Failure>) -> Reply {
    match result {
        Ok(body) => write_json(200, &body),
        Err((status, body)) => write_json(status, &body),
    }
}

/// The folder project `id` of the requesting origin, for a request that has
/// passed the credential check.
#[allow(clippy::result_large_err)]
fn open(
    inner: &Inner,
    headers: &HeaderMap,
    origin: Option<&str>,
    id: &str,
) -> Result<ProjectBinding, Reply> {
    authenticate(inner, headers, origin)?;
    projects::open_binding(inner, origin, id)
}

/// The body of a request that may carry none.
#[allow(clippy::result_large_err)]
async fn optional_body<T: for<'de> serde::Deserialize<'de> + Default>(
    request: Request<Body>,
) -> Result<T, Reply> {
    if header_str(request.headers(), "content-type").is_none() {
        return Ok(T::default());
    }
    read_json_body(request).await
}

/* ---------------------------------------------------------- running Git */

/// Runs Git in `root`. A Git that is missing or too old, or a command that
/// ran out of time, is already the answer to send.
async fn run(root: &Path, limit: Limit, args: &[&str]) -> Result<Output, Failure> {
    git::run(root, limit, args)
        .await
        .map_err(|error| run_failure(error, limit))
}

async fn run_input(
    root: &Path,
    limit: Limit,
    args: &[&str],
    stdin: Vec<u8>,
) -> Result<Output, Failure> {
    git::run_input(root, limit, args, Some(stdin))
        .await
        .map_err(|error| run_failure(error, limit))
}

fn run_failure(error: RunError, limit: Limit) -> Failure {
    match error {
        RunError::Missing => refuse_because(
            409,
            "git-missing",
            "Git is not installed on this computer. Install Git 2.38 or newer.",
        ),
        RunError::TooOld(version) => refuse_because(
            409,
            "git-old",
            &format!("Git {version} is too old. The Git panel needs Git 2.38 or newer."),
        ),
        RunError::Spawn(message) => refuse(500, &message),
        RunError::Timeout if limit == Limit::Transport => network_failure(
            remote::Kind::Network,
            "Git took too long to reach the remote.",
        ),
        RunError::Timeout => refuse(500, "Git took too long to answer."),
        RunError::TooLarge => refuse(500, "Git produced more output than allowed."),
    }
}

/// What Git said, without the credentials of any URL in it.
fn words(output: &Output) -> String {
    if output.stderr.trim().is_empty() {
        validate::redact(&output.said())
    } else {
        validate::redact(&output.stderr)
    }
}

/// Git refused, and the reason is its own words.
fn declined(output: &Output) -> Failure {
    refuse(422, &words(output))
}

/// A failure of the remote, for the sign-in help and the retry.
fn network_failure(kind: remote::Kind, detail: &str) -> Failure {
    (
        502,
        json!({"reason": kind.as_str(), "detail": detail, "error": detail}),
    )
}

/// A failed command that talks to a remote: the remote's doing when what Git
/// said shows it, else Git's own refusal.
fn remote_failure(output: &Output) -> Failure {
    match remote::classify(&output.said()) {
        Some(kind) => network_failure(kind, &words(output)),
        None => declined(output),
    }
}

/* ---------------------------------------------------------- the repository */

fn has_repository(root: &Path) -> bool {
    root.join(".git").exists()
}

fn require_repository(root: &Path) -> Result<(), Failure> {
    if has_repository(root) {
        Ok(())
    } else {
        Err(refuse_because(
            409,
            "no-repository",
            "This folder is not a Git repository. Initialize it first.",
        ))
    }
}

fn detached() -> Failure {
    refuse_because(
        409,
        "detached",
        "The repository is not on a branch. Switch to a branch first.",
    )
}

fn no_remote() -> Failure {
    refuse_because(
        409,
        "no-remote",
        "This repository has no remote. Set one first.",
    )
}

async fn read_status(root: &Path) -> Result<Status, Failure> {
    let output = run(
        root,
        Limit::Local,
        &[
            "--no-optional-locks",
            "status",
            "--porcelain=v2",
            "-z",
            "--branch",
            "--untracked-files=all",
        ],
    )
    .await?;
    if !output.success() {
        return Err(declined(&output));
    }
    Ok(status::parse(&output.stdout))
}

/// The remotes of the repository as `(name, url)`, with the credentials of
/// the URLs removed.
async fn read_remotes(root: &Path) -> Result<Vec<(String, String)>, Failure> {
    let output = run(
        root,
        Limit::Local,
        &["config", "-z", "--get-regexp", r"^remote\..+\.url$"],
    )
    .await?;
    // Exit 1 means nothing matched.
    if output.code > 1 {
        return Err(declined(&output));
    }
    let mut remotes: Vec<(String, String)> = Vec::new();
    for entry in String::from_utf8_lossy(&output.stdout).split('\0') {
        let Some((key, url)) = entry.split_once('\n') else {
            continue;
        };
        let Some(name) = key
            .strip_prefix("remote.")
            .and_then(|rest| rest.strip_suffix(".url"))
        else {
            continue;
        };
        if !remotes.iter().any(|(seen, _)| seen == name) {
            remotes.push((name.to_string(), validate::display_url(url)));
        }
    }
    Ok(remotes)
}

/// Whether the repository has the name and the email a commit needs.
async fn identity_set(root: &Path) -> Result<bool, Failure> {
    let output = run(
        root,
        Limit::Local,
        &["config", "-z", "--get-regexp", r"^user\.(name|email)$"],
    )
    .await?;
    if output.code > 1 {
        return Err(declined(&output));
    }
    let (mut name, mut email) = (false, false);
    for entry in String::from_utf8_lossy(&output.stdout).split('\0') {
        let Some((key, value)) = entry.split_once('\n') else {
            continue;
        };
        if value.trim().is_empty() {
            continue;
        }
        match key {
            "user.name" => name = true,
            "user.email" => email = true,
            _ => {}
        }
    }
    Ok(name && email)
}

/// Whether a merge was started and not finished.
async fn merge_in_progress(root: &Path) -> Result<bool, Failure> {
    let output = run(
        root,
        Limit::Local,
        &["rev-parse", "-q", "--verify", "MERGE_HEAD"],
    )
    .await?;
    Ok(output.success())
}

/// The commit `HEAD` names, or `None` before the first commit.
async fn head_id(root: &Path) -> Result<Option<String>, Failure> {
    let output = run(root, Limit::Local, &["rev-parse", "-q", "--verify", "HEAD"]).await?;
    Ok(output.success().then(|| output.text()))
}

/// The files with unresolved conflicts.
async fn conflicted_paths(root: &Path) -> Result<Vec<String>, Failure> {
    let output = run(
        root,
        Limit::Local,
        &["diff", "--name-only", "--diff-filter=U", "-z"],
    )
    .await?;
    if !output.success() {
        return Err(declined(&output));
    }
    let mut paths: Vec<String> = String::from_utf8_lossy(&output.stdout)
        .split('\0')
        .filter(|path| !path.is_empty())
        .map(str::to_string)
        .collect();
    paths.sort();
    Ok(paths)
}

/// The remote a fetch, a pull or a first push talks to: the one the branch
/// follows, else `origin`, else the first one.
async fn pick_remote(
    root: &Path,
    status: &Status,
    remotes: &[(String, String)],
) -> Result<String, Failure> {
    if remotes.is_empty() {
        return Err(no_remote());
    }
    if let (Some(branch), Some(_)) = (&status.branch, &status.upstream) {
        let key = format!("branch.{branch}.remote");
        let configured = run(root, Limit::Local, &["config", "--get", &key]).await?;
        let name = configured.text();
        if configured.success() && remotes.iter().any(|(known, _)| *known == name) {
            return Ok(name);
        }
    }
    if let Some((name, _)) = remotes.iter().find(|(name, _)| name == "origin") {
        return Ok(name.clone());
    }
    Ok(remotes[0].0.clone())
}

/// Whether the project holds `path`: it is a file the editor lists, not one a
/// render wrote. The walk leaves out hidden entries and the ignored names, so
/// a change to `.gitignore` or to a build folder is not the editor's.
fn held(path: &str, render_outputs: &BTreeSet<String>) -> bool {
    protocol::safe_relative_path(path)
        && !path
            .split('/')
            .any(|part| projects::skip_reason(part).is_some())
        && !render_outputs.contains(path)
}

/// The tracked files of the project with changes not committed. Untracked
/// files are not among them: they cannot be overwritten silently, since Git
/// refuses a merge that would.
fn dirty_paths(status: &Status, render_outputs: &BTreeSet<String>) -> Vec<String> {
    status
        .changes
        .iter()
        .filter(|change| change.tracked && held(&change.path, render_outputs))
        .map(|change| change.path.clone())
        .collect()
}

fn changes_json(status: &Status) -> Vec<Value> {
    status
        .changes
        .iter()
        .map(|change| {
            let mut entry = json!({"path": change.path, "kind": change.kind.as_str()});
            if let Some(from) = &change.from {
                entry["from"] = json!(from);
            }
            entry
        })
        .collect()
}

/// The counts against the upstream, as the status and a fetch answer them.
fn counts_json(status: &Status, body: &mut Value) {
    if let Some(upstream) = &status.upstream {
        body["upstream"] = json!(upstream);
    }
    if let Some(ahead) = status.ahead {
        body["ahead"] = json!(ahead);
    }
    if let Some(behind) = status.behind {
        body["behind"] = json!(behind);
    }
}

/* --------------------------------------------------------------- status */

/// Where the folder stands with Git. Without a usable Git, or without a
/// repository, it says so and lists nothing.
async fn describe(root: &Path) -> Result<Value, Failure> {
    let repository = has_repository(root);
    let (git_info, ready) = match git::locate().await {
        git::Found::Missing => (json!({"found": false}), false),
        git::Found::TooOld(version) => (
            json!({"found": true, "version": version, "supported": false}),
            false,
        ),
        git::Found::Ready { version, .. } => (
            json!({"found": true, "version": version, "supported": true}),
            true,
        ),
    };
    if !ready || !repository {
        return Ok(json!({
            "repository": repository,
            "git": git_info,
            "remotes": [],
            "changes": [],
            "merging": false,
            "identity": false,
        }));
    }
    let (status, remotes, identity, merging) = tokio::join!(
        read_status(root),
        read_remotes(root),
        identity_set(root),
        merge_in_progress(root)
    );
    let (status, remotes, identity, merging) = (status?, remotes?, identity?, merging?);
    let remotes: Vec<Value> = remotes
        .iter()
        .map(|(name, url)| json!({"name": name, "url": url}))
        .collect();
    let mut body = json!({
        "repository": true,
        "git": git_info,
        "remotes": remotes,
        "changes": changes_json(&status),
        "merging": merging,
        "identity": identity,
    });
    if let Some(branch) = &status.branch {
        body["branch"] = json!(branch);
    }
    if status.detached {
        body["detached"] = json!(true);
    }
    counts_json(&status, &mut body);
    Ok(body)
}

pub(super) async fn handle_status(
    inner: &Arc<Inner>,
    headers: &HeaderMap,
    origin: Option<&str>,
    id: &str,
) -> Reply {
    let binding = match open(inner, headers, origin, id) {
        Ok(binding) => binding,
        Err(response) => return response,
    };
    answer(describe(&binding.root).await)
}

/* ----------------------------------------------------------------- init */

async fn run_init(root: &Path) -> Result<Value, Failure> {
    if has_repository(root) {
        return Err(refuse_because(
            409,
            "repository",
            "This folder is already a Git repository.",
        ));
    }
    let output = run(root, Limit::Local, &["init", "-b", "main"]).await?;
    if !output.success() {
        return Err(declined(&output));
    }
    Ok(json!({"repository": true}))
}

pub(super) async fn handle_init(
    inner: &Arc<Inner>,
    headers: &HeaderMap,
    origin: Option<&str>,
    id: &str,
) -> Reply {
    let binding = match open(inner, headers, origin, id) {
        Ok(binding) => binding,
        Err(response) => return response,
    };
    answer(run_init(&binding.root).await)
}

/* ----------------------------------------------------------------- diff */

/// Whether what `git cat-file` said means the file is not in `HEAD`, as
/// opposed to Git failing.
fn absent_from_head(said: &str) -> bool {
    let lower = said.to_lowercase();
    [
        "does not exist in",
        "exists on disk, but not in",
        "not a valid object name",
        "invalid object name",
    ]
    .iter()
    .any(|needle| lower.contains(needle))
}

/// The bytes of `relative` in the working tree, a link's target for a link,
/// or `None` when nothing is there. No link on the way is followed, so a link
/// cannot lead the read out of the project.
fn read_work(root: &Path, relative: &str) -> Result<Option<Vec<u8>>, Failure> {
    let mut current = root.to_path_buf();
    let mut parts = relative.split('/').peekable();
    while let Some(part) = parts.next() {
        current.push(part);
        let Ok(metadata) = std::fs::symlink_metadata(&current) else {
            return Ok(None);
        };
        if parts.peek().is_some() {
            if !metadata.is_dir() {
                return Ok(None);
            }
        } else if metadata.file_type().is_symlink() {
            return Ok(std::fs::read_link(&current)
                .ok()
                .map(|target| target.to_string_lossy().into_owned().into_bytes()));
        } else if metadata.is_file() {
            return match read_bounded_public(&current, MAX_DIFF_BYTES) {
                Ok(bytes) => Ok(Some(bytes)),
                Err(error) if error.kind() == std::io::ErrorKind::FileTooLarge => {
                    Err(too_large_to_compare())
                }
                Err(_) => Ok(None),
            };
        } else {
            return Ok(None);
        }
    }
    Ok(None)
}

fn too_large_to_compare() -> Failure {
    refuse(413, "That file is too large to compare.")
}

/// The text of one side of a diff. `Err` when the bytes are not text.
fn text_of(bytes: Option<Vec<u8>>) -> Result<Option<String>, ()> {
    let Some(bytes) = bytes else {
        return Ok(None);
    };
    if bytes[..bytes.len().min(BINARY_PROBE_BYTES)].contains(&0) {
        return Err(());
    }
    String::from_utf8(bytes).map(Some).map_err(|_| ())
}

/// `HEAD` against the working tree for one file. The committed side goes
/// through the conversions Git applies on checkout, so a project with
/// `core.autocrlf` does not show every line changed.
async fn run_diff(root: &Path, path: &str) -> Result<Value, Failure> {
    require_repository(root)?;
    if !protocol::safe_relative_path(path)
        || path
            .split('/')
            .any(|part| part.eq_ignore_ascii_case(".git"))
    {
        return Err(refuse(400, "unsafe file path"));
    }
    let spec = format!("HEAD:{path}");
    let at_head = run(root, Limit::Local, &["cat-file", "--filters", &spec]).await?;
    let head = if at_head.success() {
        Some(at_head.stdout)
    } else if absent_from_head(&at_head.said()) {
        None
    } else {
        return Err(declined(&at_head));
    };
    if head
        .as_ref()
        .is_some_and(|bytes| bytes.len() > MAX_DIFF_BYTES)
    {
        return Err(too_large_to_compare());
    }
    let (owned_root, owned_path) = (root.to_path_buf(), path.to_string());
    let work = tokio::task::spawn_blocking(move || read_work(&owned_root, &owned_path))
        .await
        .map_err(|_| refuse(500, "The file read stopped."))??;
    Ok(match (text_of(head), text_of(work)) {
        (Ok(head), Ok(work)) => json!({"head": head, "work": work, "binary": false}),
        _ => json!({"head": null, "work": null, "binary": true}),
    })
}

pub(super) async fn handle_diff(
    inner: &Arc<Inner>,
    headers: &HeaderMap,
    origin: Option<&str>,
    id: &str,
    request: Request<Body>,
) -> Reply {
    let binding = match open(inner, headers, origin, id) {
        Ok(binding) => binding,
        Err(response) => return response,
    };
    let Some(path) = query_value(request.uri(), "path") else {
        return write_json(400, &json!({"error": "a diff needs a path"}));
    };
    answer(run_diff(&binding.root, &path).await)
}

/* --------------------------------------------------------------- commit */

#[derive(serde::Deserialize)]
struct CommitBody {
    message: String,
    #[serde(default)]
    paths: Option<Vec<String>>,
}

/// Whether `path` may be named in a commit: a project path that is not inside
/// Git's own directory.
fn committable(path: &str) -> bool {
    protocol::safe_relative_path(path)
        && !path
            .split('/')
            .any(|part| part.eq_ignore_ascii_case(".git"))
}

/// The paths as a NUL separated pathspec list that matches them literally,
/// for `--pathspec-from-file`, which keeps a long list off the command line.
fn pathspec_list(paths: &[String]) -> Vec<u8> {
    let mut list = Vec::new();
    for path in paths {
        list.extend_from_slice(format!(":(literal){path}").as_bytes());
        list.push(0);
    }
    list
}

/// Stages and commits. Everything changed is committed unless `paths` names
/// the files to commit, which are then the only ones. A merge in progress is
/// finished with all of its changes. Answers the new commit's id.
async fn run_commit(
    root: &Path,
    message: &str,
    paths: Option<&[String]>,
) -> Result<Value, Failure> {
    require_repository(root)?;
    let message = message.trim();
    if message.is_empty() {
        return Err(refuse(400, "A commit needs a message."));
    }
    if message.len() > MAX_MESSAGE_BYTES || message.contains('\0') {
        return Err(refuse(400, "That commit message cannot be used."));
    }
    if let Some(paths) = paths {
        if !paths.iter().all(|path| committable(path)) {
            return Err(refuse(400, "unsafe file path"));
        }
        if paths.is_empty() {
            return Err(refuse_because(
                409,
                "nothing",
                "No file is chosen to commit.",
            ));
        }
    }
    if !identity_set(root).await? {
        return Err(refuse_because(
            409,
            "identity",
            "Git needs a name and an email before it can commit.",
        ));
    }
    let merging = merge_in_progress(root).await?;
    if let (true, Some(paths)) = (merging, paths) {
        let chosen: BTreeSet<&str> = paths.iter().map(String::as_str).collect();
        let status = read_status(root).await?;
        if status
            .changes
            .iter()
            .any(|change| !chosen.contains(change.path.as_str()))
        {
            return Err(refuse_because(
                409,
                "merging",
                "A merge is committed with every change in it.",
            ));
        }
    }
    let committed = match paths.filter(|_| !merging) {
        Some(paths) => {
            let list = pathspec_list(paths);
            let staged = run_input(
                root,
                Limit::Local,
                &["add", "-A", "--pathspec-from-file=-", "--pathspec-file-nul"],
                list.clone(),
            )
            .await?;
            if !staged.success() {
                return Err(declined(&staged));
            }
            run_input(
                root,
                Limit::Local,
                &[
                    "commit",
                    "--only",
                    "-m",
                    message,
                    "--pathspec-from-file=-",
                    "--pathspec-file-nul",
                ],
                list,
            )
            .await?
        }
        None => {
            let staged = run(root, Limit::Local, &["add", "-A"]).await?;
            if !staged.success() {
                return Err(declined(&staged));
            }
            run(root, Limit::Local, &["commit", "-m", message]).await?
        }
    };
    if !committed.success() {
        let said = committed.said();
        if said.contains("nothing to commit") || said.contains("no changes added to commit") {
            return Err(refuse_because(
                409,
                "nothing",
                "There is nothing to commit.",
            ));
        }
        return Err(declined(&committed));
    }
    let head = head_id(root).await?.unwrap_or_default();
    Ok(json!({"commit": head}))
}

pub(super) async fn handle_commit(
    inner: &Arc<Inner>,
    headers: &HeaderMap,
    origin: Option<&str>,
    id: &str,
    request: Request<Body>,
) -> Reply {
    let binding = match open(inner, headers, origin, id) {
        Ok(binding) => binding,
        Err(response) => return response,
    };
    let body = match read_json_body::<CommitBody>(request).await {
        Ok(body) => body,
        Err(response) => return response,
    };
    answer(run_commit(&binding.root, &body.message, body.paths.as_deref()).await)
}

/* ------------------------------------------------------------- identity */

#[derive(serde::Deserialize)]
struct IdentityBody {
    name: String,
    email: String,
}

/// Writes the name and the email into the repository's own configuration.
async fn run_identity(root: &Path, name: &str, email: &str) -> Result<Value, Failure> {
    require_repository(root)?;
    let (name, email) = (name.trim(), email.trim());
    if !validate::valid_identity_field(name) || !validate::valid_identity_field(email) {
        return Err(refuse(
            400,
            "A name and an email are needed, each on one line.",
        ));
    }
    for (key, value) in [("user.name", name), ("user.email", email)] {
        let output = run(root, Limit::Local, &["config", "--local", key, value]).await?;
        if !output.success() {
            return Err(declined(&output));
        }
    }
    Ok(json!({"identity": true}))
}

pub(super) async fn handle_identity(
    inner: &Arc<Inner>,
    headers: &HeaderMap,
    origin: Option<&str>,
    id: &str,
    request: Request<Body>,
) -> Reply {
    let binding = match open(inner, headers, origin, id) {
        Ok(binding) => binding,
        Err(response) => return response,
    };
    let body = match read_json_body::<IdentityBody>(request).await {
        Ok(body) => body,
        Err(response) => return response,
    };
    answer(run_identity(&binding.root, &body.name, &body.email).await)
}

/* ------------------------------------------------------------- branches */

/// The local branches, the current one, and the branches only a remote has.
async fn run_branches(root: &Path) -> Result<Value, Failure> {
    require_repository(root)?;
    let refs = run(
        root,
        Limit::Local,
        &[
            "for-each-ref",
            "--format=%(refname)",
            "refs/heads",
            "refs/remotes",
        ],
    )
    .await?;
    if !refs.success() {
        return Err(declined(&refs));
    }
    let (mut local, mut remote_only) = (BTreeSet::new(), BTreeSet::new());
    for line in refs.text().lines() {
        if let Some(name) = line.strip_prefix("refs/heads/") {
            local.insert(name.to_string());
        } else if let Some((_, name)) = line
            .strip_prefix("refs/remotes/")
            .and_then(|rest| rest.split_once('/'))
        {
            if name != "HEAD" {
                remote_only.insert(name.to_string());
            }
        }
    }
    remote_only.retain(|name| !local.contains(name));
    let head = run(
        root,
        Limit::Local,
        &["symbolic-ref", "-q", "--short", "HEAD"],
    )
    .await?;
    let current = head.success().then(|| head.text());
    Ok(json!({"current": current, "branches": local, "remote": remote_only}))
}

pub(super) async fn handle_branches(
    inner: &Arc<Inner>,
    headers: &HeaderMap,
    origin: Option<&str>,
    id: &str,
) -> Reply {
    let binding = match open(inner, headers, origin, id) {
        Ok(binding) => binding,
        Err(response) => return response,
    };
    answer(run_branches(&binding.root).await)
}

#[derive(serde::Deserialize)]
struct BranchBody {
    branch: String,
}

fn checked_branch(branch: &str) -> Result<&str, Failure> {
    let branch = branch.trim();
    if validate::valid_branch(branch) {
        Ok(branch)
    } else {
        Err(refuse(400, "That is not a valid branch name."))
    }
}

/// Switches to a branch that exists, or to the one a single remote has under
/// that name.
async fn run_switch(root: &Path, branch: &str) -> Result<Value, Failure> {
    require_repository(root)?;
    let branch = checked_branch(branch)?;
    let output = run(root, Limit::Local, &["switch", branch]).await?;
    if !output.success() {
        return Err(declined(&output));
    }
    Ok(json!({"branch": branch}))
}

/// Creates a branch at `HEAD` and switches to it. Before the first commit
/// there is no `HEAD` to branch from, so the branch `HEAD` is on is renamed.
async fn run_create_branch(root: &Path, branch: &str) -> Result<Value, Failure> {
    require_repository(root)?;
    let branch = checked_branch(branch)?;
    let output = if head_id(root).await?.is_some() {
        run(root, Limit::Local, &["switch", "-c", branch]).await?
    } else {
        let reference = format!("refs/heads/{branch}");
        run(root, Limit::Local, &["symbolic-ref", "HEAD", &reference]).await?
    };
    if !output.success() {
        return Err(declined(&output));
    }
    Ok(json!({"branch": branch}))
}

pub(super) async fn handle_switch(
    inner: &Arc<Inner>,
    headers: &HeaderMap,
    origin: Option<&str>,
    id: &str,
    request: Request<Body>,
) -> Reply {
    let binding = match open(inner, headers, origin, id) {
        Ok(binding) => binding,
        Err(response) => return response,
    };
    let body = match read_json_body::<BranchBody>(request).await {
        Ok(body) => body,
        Err(response) => return response,
    };
    answer(run_switch(&binding.root, &body.branch).await)
}

pub(super) async fn handle_branch(
    inner: &Arc<Inner>,
    headers: &HeaderMap,
    origin: Option<&str>,
    id: &str,
    request: Request<Body>,
) -> Reply {
    let binding = match open(inner, headers, origin, id) {
        Ok(binding) => binding,
        Err(response) => return response,
    };
    let body = match read_json_body::<BranchBody>(request).await {
        Ok(body) => body,
        Err(response) => return response,
    };
    answer(run_create_branch(&binding.root, &body.branch).await)
}

/* --------------------------------------------------------------- remote */

#[derive(serde::Deserialize)]
struct RemoteBody {
    name: String,
    url: String,
}

/// Adds the remote `name`, or points it at `url` when it exists.
async fn run_set_remote(root: &Path, name: &str, url: &str) -> Result<Value, Failure> {
    require_repository(root)?;
    let name = name.trim();
    if !validate::valid_remote_name(name) {
        return Err(refuse(400, "That is not a valid remote name."));
    }
    let url = validate::validate_url(url).map_err(|why| refuse(400, &why))?;
    let known = read_remotes(root)
        .await?
        .iter()
        .any(|(known, _)| known == name);
    let verb = if known { "set-url" } else { "add" };
    let output = run(root, Limit::Local, &["remote", verb, name, url.as_str()]).await?;
    if !output.success() {
        return Err(declined(&output));
    }
    Ok(json!({"name": name, "url": validate::display_url(&url)}))
}

pub(super) async fn handle_remote(
    inner: &Arc<Inner>,
    headers: &HeaderMap,
    origin: Option<&str>,
    id: &str,
    request: Request<Body>,
) -> Reply {
    let binding = match open(inner, headers, origin, id) {
        Ok(binding) => binding,
        Err(response) => return response,
    };
    let body = match read_json_body::<RemoteBody>(request).await {
        Ok(body) => body,
        Err(response) => return response,
    };
    answer(run_set_remote(&binding.root, &body.name, &body.url).await)
}

/* ---------------------------------------------------------------- fetch */

/// Fetches the remote the branch follows and answers how far the branch is
/// from its upstream.
async fn run_fetch(root: &Path) -> Result<Value, Failure> {
    require_repository(root)?;
    let remotes = read_remotes(root).await?;
    let status = read_status(root).await?;
    let remote = pick_remote(root, &status, &remotes).await?;
    let output = run(root, Limit::Transport, &["fetch", "--", &remote]).await?;
    if !output.success() {
        return Err(remote_failure(&output));
    }
    let status = read_status(root).await?;
    let mut body = json!({});
    counts_json(&status, &mut body);
    Ok(body)
}

pub(super) async fn handle_fetch(
    inner: &Arc<Inner>,
    headers: &HeaderMap,
    origin: Option<&str>,
    id: &str,
) -> Reply {
    let binding = match open(inner, headers, origin, id) {
        Ok(binding) => binding,
        Err(response) => return response,
    };
    answer(run_fetch(&binding.root).await)
}

/* ----------------------------------------------------------------- pull */

/// `git pull --no-rebase`. The project's tracked files must be committed
/// first, and a merge in progress must be finished. A branch with no upstream
/// pulls the branch of that name from its remote, and finds nothing to pull
/// when the remote has none.
async fn run_pull(root: &Path, render_outputs: &BTreeSet<String>) -> Result<Value, Failure> {
    require_repository(root)?;
    let status = read_status(root).await?;
    if merge_in_progress(root).await? {
        return Err(refuse_because(
            409,
            "merging",
            "Finish the merge in progress first: resolve the conflicts, then commit.",
        ));
    }
    let dirty = dirty_paths(&status, render_outputs);
    if !dirty.is_empty() {
        return Err((
            409,
            json!({
                "reason": "dirty",
                "paths": dirty,
                "error": "Commit your changes to tracked files first.",
            }),
        ));
    }
    let named = match (&status.upstream, &status.branch) {
        (Some(_), _) => None,
        (None, Some(branch)) => {
            let remotes = read_remotes(root).await?;
            Some((pick_remote(root, &status, &remotes).await?, branch.clone()))
        }
        (None, None) => return Err(detached()),
    };
    let before = head_id(root).await?;
    let mut args = vec!["pull", "--no-rebase", "--no-edit"];
    if let Some((remote, branch)) = &named {
        args.extend([remote.as_str(), branch.as_str()]);
    }
    let output = run(root, Limit::Transport, &args).await?;
    if output.success() {
        let after = head_id(root).await?;
        let state = if after == before {
            "up-to-date"
        } else if before.is_none() || output.said().contains("Fast-forward") {
            "fast-forward"
        } else {
            "merged"
        };
        return Ok(json!({"status": state, "conflicts": []}));
    }
    let conflicts = conflicted_paths(root).await?;
    if !conflicts.is_empty() {
        return Ok(json!({"status": "conflicts", "conflicts": conflicts}));
    }
    if named.is_some() && output.said().contains("couldn't find remote ref") {
        return Ok(json!({"status": "up-to-date", "conflicts": []}));
    }
    Err(remote_failure(&output))
}

pub(super) async fn handle_pull(
    inner: &Arc<Inner>,
    headers: &HeaderMap,
    origin: Option<&str>,
    id: &str,
) -> Reply {
    let binding = match open(inner, headers, origin, id) {
        Ok(binding) => binding,
        Err(response) => return response,
    };
    let render_outputs = inner.projects.render_outputs(&binding.id);
    answer(run_pull(&binding.root, &render_outputs).await)
}

/* ----------------------------------------------------------------- push */

/// `git push` of the current branch, never forced and never after a pull. The
/// first push of a branch sets its upstream.
async fn run_push(root: &Path) -> Result<Value, Failure> {
    require_repository(root)?;
    let status = read_status(root).await?;
    let Some(branch) = status.branch.clone() else {
        return Err(detached());
    };
    let remotes = read_remotes(root).await?;
    let remote = pick_remote(root, &status, &remotes).await?;
    let mut args = vec!["push"];
    if status.upstream.is_none() {
        args.extend(["-u", remote.as_str(), branch.as_str()]);
    }
    let output = run(root, Limit::Transport, &args).await?;
    if output.success() {
        let state = if output
            .said()
            .to_lowercase()
            .contains("everything up-to-date")
        {
            "up-to-date"
        } else {
            "pushed"
        };
        return Ok(json!({"status": state, "branch": branch, "remote": remote}));
    }
    if remote::behind(&output.said()) {
        return Err(refuse_because(
            409,
            "behind",
            "The remote has commits you do not have. Pull first.",
        ));
    }
    Err(remote_failure(&output))
}

pub(super) async fn handle_push(
    inner: &Arc<Inner>,
    headers: &HeaderMap,
    origin: Option<&str>,
    id: &str,
) -> Reply {
    let binding = match open(inner, headers, origin, id) {
        Ok(binding) => binding,
        Err(response) => return response,
    };
    answer(run_push(&binding.root).await)
}

/* ----------------------------------------------------------------- sync */

#[derive(serde::Deserialize, Default)]
struct SyncBody {
    #[serde(default)]
    message: Option<String>,
}

/// Records the outcome of one step and says whether the next one may run. A
/// successful step carries the fields its closure answers with. A failed one
/// carries its reason and detail, and the paths, conflicts and status that the
/// refusal names when it has them.
fn record(steps: &mut Vec<Value>, step: &str, outcome: Result<Value, Failure>) -> bool {
    match outcome {
        Ok(fields) => {
            let mut entry = json!({"step": step, "ok": true});
            if let Some(fields) = fields.as_object() {
                for (key, value) in fields {
                    entry[key] = value.clone();
                }
            }
            steps.push(entry);
            true
        }
        Err((_, body)) => {
            let mut entry = json!({"step": step, "ok": false});
            for key in ["reason", "paths", "conflicts", "status"] {
                if let Some(value) = body.get(key) {
                    entry[key] = value.clone();
                }
            }
            let detail = body
                .get("detail")
                .or_else(|| body.get("error"))
                .and_then(Value::as_str)
                .unwrap_or("The step failed.");
            entry["detail"] = json!(detail);
            steps.push(entry);
            false
        }
    }
}

/// Commits when the tree is dirty, pulls, then pushes, and stops at the first
/// step that fails. A tree with nothing to commit has no commit step. A step
/// that succeeds answers the commit's full id, or the status of the pull or
/// the push.
async fn run_sync(
    root: &Path,
    render_outputs: &BTreeSet<String>,
    message: Option<&str>,
) -> Result<Value, Failure> {
    require_repository(root)?;
    let status = read_status(root).await?;
    let merging = merge_in_progress(root).await?;
    let mut steps: Vec<Value> = Vec::new();
    if !status.changes.is_empty() || merging {
        let message = message
            .map(str::trim)
            .filter(|message| !message.is_empty())
            .unwrap_or(SYNC_MESSAGE);
        let committed = run_commit(root, message, None).await;
        if !record(&mut steps, "commit", committed) {
            return Ok(json!({"steps": steps}));
        }
    }
    let pulled = run_pull(root, render_outputs).await.and_then(|body| {
        if body["status"] == "conflicts" {
            let count = body["conflicts"].as_array().map_or(0, Vec::len);
            let files = if count == 1 {
                "1 file".to_string()
            } else {
                format!("{count} files")
            };
            Err((
                409,
                json!({
                    "reason": "conflicts",
                    "status": "conflicts",
                    "conflicts": body["conflicts"],
                    "error": format!(
                        "The merge has conflicts in {files}. \
                         Resolve them in the editor, then commit."
                    ),
                }),
            ))
        } else {
            Ok(json!({"status": body["status"]}))
        }
    });
    if !record(&mut steps, "pull", pulled) {
        return Ok(json!({"steps": steps}));
    }
    let pushed = run_push(root)
        .await
        .map(|body| json!({"status": body["status"]}));
    record(&mut steps, "push", pushed);
    Ok(json!({"steps": steps}))
}

pub(super) async fn handle_sync(
    inner: &Arc<Inner>,
    headers: &HeaderMap,
    origin: Option<&str>,
    id: &str,
    request: Request<Body>,
) -> Reply {
    let binding = match open(inner, headers, origin, id) {
        Ok(binding) => binding,
        Err(response) => return response,
    };
    let body = match optional_body::<SyncBody>(request).await {
        Ok(body) => body,
        Err(response) => return response,
    };
    let render_outputs = inner.projects.render_outputs(&binding.id);
    answer(run_sync(&binding.root, &render_outputs, body.message.as_deref()).await)
}

/* ---------------------------------------------------------------- clone */

#[derive(serde::Deserialize)]
struct CloneBody {
    url: String,
}

/// Clones `url` into `<parent>/<name>`, fetches the state LibrePaper keeps in
/// `refs/librepaper/*` when the remote has any, and opens the new folder as a
/// project of `origin`. Answers like `projects/open`.
async fn run_clone(
    inner: &Inner,
    origin: Option<&str>,
    url: &str,
    name: &str,
    parent: &Path,
) -> Result<Value, Failure> {
    let target = parent.join(name);
    if std::fs::symlink_metadata(&target).is_ok() {
        return Err(refuse_because(
            409,
            "exists",
            &format!("{name} already exists in that folder."),
        ));
    }
    let output = run(parent, Limit::Transport, &["clone", "--", url, name]).await?;
    if !output.success() {
        return Err(remote_failure(&output));
    }
    // The state is there only when the repository was kept with LibrePaper
    // before, and a clone without it is just as good.
    let _ = git::run(
        &target,
        Limit::Transport,
        &["fetch", "--", url, "+refs/librepaper/*:refs/librepaper/*"],
    )
    .await;
    let binding = inner
        .bindings
        .open_project(origin.unwrap_or_default(), &target)
        .map_err(|error| refuse(400, &error))?;
    Ok(projects::opened(&binding))
}

pub(super) async fn handle_clone(
    inner: &Arc<Inner>,
    headers: &HeaderMap,
    origin: Option<&str>,
    request: Request<Body>,
) -> Reply {
    if let Err(response) = authenticate(inner, headers, origin) {
        return response;
    }
    let body = match read_json_body::<CloneBody>(request).await {
        Ok(body) => body,
        Err(response) => return response,
    };
    let url = match validate::validate_url(&body.url) {
        Ok(url) => url,
        Err(why) => return write_json(400, &json!({"error": why})),
    };
    let Some(name) = validate::clone_name(&url) else {
        return write_json(
            400,
            &json!({"error": "A folder name cannot be made from that URL."}),
        );
    };
    let parent = match projects::choose_root(inner, "Choose where to put the clone").await {
        Ok(parent) => parent,
        Err(response) => return response,
    };
    answer(run_clone(inner, origin, &url, &name, &parent).await)
}

#[cfg(test)]
mod tests {
    use super::*;

    const PAGE: &str = "https://paper.example";

    /// Whether this machine has a Git the routes accept. The tests that run
    /// Git return at once without one.
    async fn git_ready() -> bool {
        matches!(git::locate().await, git::Found::Ready { .. })
    }

    /// Plain Git for test setup, away from the configuration of whoever runs
    /// the tests, with an identity that is not the project's. Answers stdout.
    fn plain_git(dir: &Path, args: &[&str]) -> String {
        let mut command = std::process::Command::new(git::find().expect("git"));
        command
            .arg("-C")
            .arg(dir)
            .args([
                "-c",
                "user.name=Other",
                "-c",
                "user.email=other@example.org",
            ])
            .args(args);
        for name in git::AMBIENT {
            command.env_remove(name);
        }
        command
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env(
                "GIT_CONFIG_GLOBAL",
                if cfg!(windows) { "NUL" } else { "/dev/null" },
            )
            .env("GIT_TERMINAL_PROMPT", "0")
            .env("LC_ALL", "C");
        let output = command.output().expect("git runs");
        assert!(
            output.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).trim().to_string()
    }

    fn text(path: &Path) -> &str {
        path.to_str().expect("a path that is text")
    }

    /// A folder project of `PAGE` and a scratch directory for the other
    /// repositories a test needs.
    struct Fixture {
        inner: Arc<Inner>,
        binding: ProjectBinding,
        scratch: tempfile::TempDir,
        _state: tempfile::TempDir,
        _root: tempfile::TempDir,
    }

    impl Fixture {
        async fn new() -> Fixture {
            let state = tempfile::tempdir().unwrap();
            let inner = test_support::inner(state.path(), state.path()).await;
            let root = tempfile::tempdir().unwrap();
            let binding = inner.bindings.open_project(PAGE, root.path()).unwrap();
            Fixture {
                inner,
                binding,
                scratch: tempfile::tempdir().unwrap(),
                _state: state,
                _root: root,
            }
        }

        fn root(&self) -> &Path {
            &self.binding.root
        }

        fn write(&self, relative: &str, content: &str) {
            let path = self.root().join(relative);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, content).unwrap();
        }

        fn read(&self, relative: &str) -> String {
            std::fs::read_to_string(self.root().join(relative)).unwrap()
        }

        /// One request through the router, as the desktop shell sends it.
        /// `tail` is the path under the routes' base.
        async fn send(
            &self,
            method: Method,
            tail: &str,
            origin: &str,
            credential: bool,
            body: Option<Value>,
        ) -> (u16, Value) {
            let mut builder = Request::builder()
                .method(method)
                .uri(format!("{BASE_PATH}/{tail}"))
                .header("host", format!("127.0.0.1:{}", test_support::PORT))
                .header("x-librepaper-origin", origin);
            if credential {
                builder = builder.header(
                    "authorization",
                    format!("Bearer {}", test_support::credential(&self.inner)),
                );
            }
            let request = match body {
                Some(body) => builder
                    .header("content-type", "application/json")
                    .body(Body::from(body.to_string())),
                None => builder.body(Body::empty()),
            }
            .unwrap();
            let peer: SocketAddr = "127.0.0.1:12345".parse().unwrap();
            let reply = handle(
                axum::extract::State(self.inner.clone()),
                ConnectInfo(peer),
                request,
            )
            .await;
            read(reply).await
        }

        /// A route under this folder project's `git`.
        async fn route(&self, method: Method, path: &str, body: Option<Value>) -> (u16, Value) {
            let tail = format!("projects/{}/git{path}", self.binding.id);
            self.send(method, &tail, PAGE, true, body).await
        }

        async fn get(&self, path: &str) -> (u16, Value) {
            self.route(Method::GET, path, None).await
        }

        async fn post(&self, path: &str, body: Value) -> (u16, Value) {
            self.route(Method::POST, path, Some(body)).await
        }

        /// A POST that carries no body.
        async fn post_empty(&self, path: &str) -> (u16, Value) {
            self.route(Method::POST, path, None).await
        }

        /// The answer to `GET git`.
        async fn status(&self) -> Value {
            let (status, body) = self.get("").await;
            assert_eq!(status, 200, "{body}");
            body
        }

        /// Initializes the repository and gives it an identity.
        async fn init(&self) {
            assert_eq!(self.post_empty("/init").await.0, 200);
            let identity = json!({"name": "Ada", "email": "ada@example.org"});
            assert_eq!(self.post("/identity", identity).await.0, 200);
        }

        /// Commits everything and answers the commit's id.
        async fn commit_all(&self, message: &str) -> String {
            let (status, body) = self.post("/commit", json!({"message": message})).await;
            assert_eq!(status, 200, "{body}");
            body["commit"].as_str().unwrap().to_string()
        }

        /// A bare repository holding the first commit of this project, which
        /// follows it as `origin`. `a.txt` holds three lines.
        async fn with_remote(&self) -> PathBuf {
            let bare = self.scratch.path().join("remote.git");
            plain_git(
                self.scratch.path(),
                &["init", "--bare", "-b", "main", "remote.git"],
            );
            self.init().await;
            self.write("a.txt", "one\ntwo\nthree\n");
            self.commit_all("First").await;
            plain_git(self.root(), &["remote", "add", "origin", text(&bare)]);
            let (status, body) = self.post_empty("/push").await;
            assert_eq!((status, &body["status"]), (200, &json!("pushed")), "{body}");
            bare
        }

        /// A second clone of `bare`, for someone else's commits.
        fn other(&self, bare: &Path) -> PathBuf {
            let other = self.scratch.path().join("other");
            plain_git(self.scratch.path(), &["clone", text(bare), "other"]);
            other
        }
    }

    /// Commits `content` to `file` in `other` and pushes it.
    fn commit_elsewhere(other: &Path, file: &str, content: &str, message: &str) {
        std::fs::write(other.join(file), content).unwrap();
        plain_git(other, &["add", "-A"]);
        plain_git(other, &["commit", "-m", message]);
        plain_git(other, &["push", "origin", "main"]);
    }

    async fn read(reply: Reply) -> (u16, Value) {
        let status = reply.status().as_u16();
        let bytes = axum::body::to_bytes(reply.into_body(), 1 << 20)
            .await
            .unwrap();
        (
            status,
            serde_json::from_slice(&bytes).unwrap_or(Value::Null),
        )
    }

    fn json_request(body: Value) -> Request<Body> {
        Request::post("/git")
            .header("content-type", "application/json")
            .body(Body::from(body.to_string()))
            .unwrap()
    }

    #[tokio::test]
    async fn a_plain_folder_is_not_a_repository() {
        if !git_ready().await {
            return;
        }
        let fixture = Fixture::new().await;
        fixture.write("main.tex", "x");
        let body = fixture.status().await;
        assert_eq!(body["repository"], false);
        assert_eq!(body["git"]["found"], true);
        assert!(body["git"]["version"].is_string());
        assert_eq!(body["git"]["supported"], true);
        assert_eq!(body["remotes"], json!([]));
        assert_eq!(body["changes"], json!([]));
        assert_eq!(body["merging"], false);
        assert_eq!(body["identity"], false);
        for (method, path, request) in [
            (Method::POST, "/commit", Some(json!({"message": "x"}))),
            (
                Method::POST,
                "/identity",
                Some(json!({"name": "a", "email": "b"})),
            ),
            (Method::POST, "/switch", Some(json!({"branch": "main"}))),
            (Method::POST, "/branch", Some(json!({"branch": "dev"}))),
            (
                Method::POST,
                "/remote",
                Some(json!({"name": "origin", "url": "https://a.example/o/r.git"})),
            ),
            (Method::POST, "/fetch", None),
            (Method::POST, "/pull", None),
            (Method::POST, "/push", None),
            (Method::POST, "/sync", Some(json!({}))),
            (Method::GET, "/branches", None),
            (Method::GET, "/diff?path=main.tex", None),
        ] {
            let (status, body) = fixture.route(method, path, request).await;
            assert_eq!(
                (status, &body["reason"]),
                (409, &json!("no-repository")),
                "{path}"
            );
        }
    }

    #[tokio::test]
    async fn initializing_makes_a_repository_on_main() {
        if !git_ready().await {
            return;
        }
        let fixture = Fixture::new().await;
        fixture.write("main.tex", "x");
        let (status, body) = fixture.post_empty("/init").await;
        assert_eq!((status, &body["repository"]), (200, &json!(true)), "{body}");
        assert!(fixture.root().join(".git").exists());

        let body = fixture.status().await;
        assert_eq!(body["repository"], true);
        assert_eq!(body["branch"], "main");
        assert_eq!(
            body["changes"],
            json!([{"path": "main.tex", "kind": "added"}])
        );
        assert_eq!(body["merging"], false);
        assert!(body.get("upstream").is_none());
        assert!(body.get("ahead").is_none());

        let (status, body) = fixture.post_empty("/init").await;
        assert_eq!((status, &body["reason"]), (409, &json!("repository")));
    }

    #[tokio::test]
    async fn a_commit_needs_an_identity_and_then_commits() {
        if !git_ready().await {
            return;
        }
        let fixture = Fixture::new().await;
        fixture.post_empty("/init").await;
        fixture.write("a.txt", "one\n");

        let first = json!({"message": "First"});
        let (status, body) = fixture.post("/commit", first.clone()).await;
        assert_eq!(
            (status, &body["reason"]),
            (409, &json!("identity")),
            "{body}"
        );
        assert_eq!(fixture.status().await["identity"], false);

        for bad in [
            json!({"name": "", "email": "ada@example.org"}),
            json!({"name": "Ada", "email": ""}),
            json!({"name": "Ada\nLovelace", "email": "ada@example.org"}),
            json!({"name": "-Ada", "email": "ada@example.org"}),
        ] {
            assert_eq!(fixture.post("/identity", bad).await.0, 400);
        }
        let identity = json!({"name": "Ada", "email": "ada@example.org"});
        let (status, body) = fixture.post("/identity", identity).await;
        assert_eq!((status, &body["identity"]), (200, &json!(true)), "{body}");
        assert_eq!(fixture.status().await["identity"], true);

        let (status, body) = fixture.post("/commit", first).await;
        assert_eq!(status, 200, "{body}");
        let id = body["commit"].as_str().unwrap();
        assert_eq!(id.len(), 40);
        assert_eq!(plain_git(fixture.root(), &["rev-parse", "HEAD"]), id);
        assert_eq!(
            plain_git(fixture.root(), &["log", "-1", "--format=%s|%an|%ae"]),
            "First|Ada|ada@example.org"
        );
        assert_eq!(fixture.status().await["changes"], json!([]));

        let (status, body) = fixture.post("/commit", json!({"message": "Again"})).await;
        assert_eq!(
            (status, &body["reason"]),
            (409, &json!("nothing")),
            "{body}"
        );
        let (status, _) = fixture.post("/commit", json!({"message": "  "})).await;
        assert_eq!(status, 400);
    }

    #[tokio::test]
    async fn a_commit_can_leave_files_out() {
        if !git_ready().await {
            return;
        }
        let fixture = Fixture::new().await;
        fixture.init().await;
        fixture.write("a.txt", "one\n");
        fixture.write("b.txt", "two\n");
        fixture.write("dir/c [1].txt", "three\n");

        let only = json!({"message": "Only some", "paths": ["a.txt", "dir/c [1].txt"]});
        let (status, body) = fixture.post("/commit", only).await;
        assert_eq!(status, 200, "{body}");
        assert_eq!(
            plain_git(
                fixture.root(),
                &[
                    "diff-tree",
                    "--no-commit-id",
                    "--name-only",
                    "-r",
                    "--root",
                    "HEAD"
                ]
            ),
            "a.txt\ndir/c [1].txt"
        );
        assert_eq!(
            fixture.status().await["changes"],
            json!([{"path": "b.txt", "kind": "added"}])
        );

        for (paths, status, reason) in [
            (json!([]), 409, Some("nothing")),
            (json!(["../outside"]), 400, None),
            (json!([".git/config"]), 400, None),
        ] {
            let body = json!({"message": "x", "paths": paths});
            let (got, answer) = fixture.post("/commit", body).await;
            assert_eq!(got, status, "{answer}");
            if let Some(reason) = reason {
                assert_eq!(answer["reason"], reason);
            }
        }
    }

    #[tokio::test]
    async fn status_names_what_changed() {
        if !git_ready().await {
            return;
        }
        let fixture = Fixture::new().await;
        fixture.init().await;
        for name in ["a.txt", "b.txt", "c.txt"] {
            fixture.write(name, "same\n");
        }
        fixture.commit_all("Three files").await;
        fixture.write("a.txt", "different\n");
        std::fs::remove_file(fixture.root().join("b.txt")).unwrap();
        plain_git(fixture.root(), &["mv", "c.txt", "d.txt"]);
        fixture.write("e.txt", "new\n");

        let body = fixture.status().await;
        assert_eq!(
            body["changes"],
            json!([
                {"path": "a.txt", "kind": "modified"},
                {"path": "b.txt", "kind": "deleted"},
                {"path": "d.txt", "kind": "renamed", "from": "c.txt"},
                {"path": "e.txt", "kind": "added"},
            ])
        );
    }

    #[tokio::test]
    async fn branches_can_be_listed_created_and_switched() {
        if !git_ready().await {
            return;
        }
        let fixture = Fixture::new().await;
        fixture.init().await;

        // Before the first commit the branch is renamed.
        let (status, body) = fixture.post("/branch", json!({"branch": "draft"})).await;
        assert_eq!((status, &body["branch"]), (200, &json!("draft")), "{body}");
        assert_eq!(fixture.status().await["branch"], "draft");
        let (status, _) = fixture.post("/switch", json!({"branch": "main"})).await;
        assert_eq!(status, 422);

        fixture.write("a.txt", "one\n");
        fixture.commit_all("First").await;
        let (_, body) = fixture.get("/branches").await;
        assert_eq!(
            body,
            json!({"current": "draft", "branches": ["draft"], "remote": []})
        );

        let (status, body) = fixture.post("/branch", json!({"branch": "dev"})).await;
        assert_eq!((status, &body["branch"]), (200, &json!("dev")), "{body}");
        assert_eq!(fixture.status().await["branch"], "dev");
        let (status, _) = fixture.post("/switch", json!({"branch": "draft"})).await;
        assert_eq!(status, 200);
        let (_, body) = fixture.get("/branches").await;
        assert_eq!(body["current"], "draft");
        assert_eq!(body["branches"], json!(["dev", "draft"]));

        for bad in ["-x", "a b", "a..b", "", "x.lock"] {
            for path in ["/switch", "/branch"] {
                let (status, _) = fixture.post(path, json!({"branch": bad})).await;
                assert_eq!(status, 400, "{path} {bad:?}");
            }
        }
        let (status, _) = fixture.post("/switch", json!({"branch": "missing"})).await;
        assert_eq!(status, 422);
        let (status, _) = fixture.post("/branch", json!({"branch": "dev"})).await;
        assert_eq!(status, 422);
    }

    #[tokio::test]
    async fn the_remote_is_set_and_changed_with_checked_urls() {
        if !git_ready().await {
            return;
        }
        let fixture = Fixture::new().await;
        fixture.init().await;

        let set = json!({"name": "origin", "url": "https://github.com/o/r.git"});
        let (status, body) = fixture.post("/remote", set.clone()).await;
        assert_eq!(status, 200, "{body}");
        assert_eq!(body, set);
        assert_eq!(fixture.status().await["remotes"], json!([set]));

        let moved = json!({"name": "origin", "url": "git@github.com:o/r.git"});
        assert_eq!(fixture.post("/remote", moved.clone()).await.0, 200);
        let token = json!({"name": "fork", "url": "https://secret-token@github.com/me/r.git"});
        let (status, body) = fixture.post("/remote", token).await;
        assert_eq!(status, 200, "{body}");
        assert_eq!(body["url"], "https://github.com/me/r.git");
        assert_eq!(
            fixture.status().await["remotes"],
            json!([
                moved,
                {"name": "fork", "url": "https://github.com/me/r.git"},
            ])
        );

        for (name, url) in [
            ("origin", "https://user:secret@github.com/o/r.git"),
            ("origin", "http://github.com/o/r.git"),
            ("origin", "/tmp/some/repository"),
            ("origin", "file:///tmp/some/repository"),
            ("origin", "ext::sh -c id"),
            ("origin", "-oProxyCommand=x"),
            ("origin", ""),
            ("", "https://github.com/o/r.git"),
            ("-x", "https://github.com/o/r.git"),
            ("a b", "https://github.com/o/r.git"),
        ] {
            let (status, body) = fixture
                .post("/remote", json!({"name": name, "url": url}))
                .await;
            assert_eq!(status, 400, "{name:?} {url:?} {body}");
        }
    }

    #[tokio::test]
    async fn the_first_push_sets_the_upstream() {
        if !git_ready().await {
            return;
        }
        let fixture = Fixture::new().await;
        let bare = fixture.scratch.path().join("remote.git");
        plain_git(
            fixture.scratch.path(),
            &["init", "--bare", "-b", "main", "remote.git"],
        );
        fixture.init().await;
        fixture.write("a.txt", "one\n");
        let (status, body) = fixture.post_empty("/push").await;
        assert_eq!(
            (status, &body["reason"]),
            (409, &json!("no-remote")),
            "{body}"
        );
        let id = fixture.commit_all("First").await;
        plain_git(fixture.root(), &["remote", "add", "origin", text(&bare)]);
        assert!(fixture.status().await.get("upstream").is_none());

        let (status, body) = fixture.post_empty("/push").await;
        assert_eq!(status, 200, "{body}");
        assert_eq!(
            body,
            json!({"status": "pushed", "branch": "main", "remote": "origin"})
        );
        assert_eq!(plain_git(&bare, &["rev-parse", "main"]), id);
        let status = fixture.status().await;
        assert_eq!(status["upstream"], "origin/main");
        assert_eq!(
            (&status["ahead"], &status["behind"]),
            (&json!(0), &json!(0))
        );

        let (status, body) = fixture.post_empty("/push").await;
        assert_eq!(
            (status, &body["status"]),
            (200, &json!("up-to-date")),
            "{body}"
        );
    }

    #[tokio::test]
    async fn a_push_behind_the_remote_is_refused_until_the_branch_is_pulled() {
        if !git_ready().await {
            return;
        }
        let fixture = Fixture::new().await;
        let bare = fixture.with_remote().await;
        let other = fixture.other(&bare);
        commit_elsewhere(&other, "b.txt", "two\n", "Add b");
        fixture.write("c.txt", "three\n");
        fixture.commit_all("Add c").await;

        let (status, body) = fixture.post_empty("/push").await;
        assert_eq!((status, &body["reason"]), (409, &json!("behind")), "{body}");

        let (status, body) = fixture.post_empty("/pull").await;
        assert_eq!((status, &body["status"]), (200, &json!("merged")), "{body}");
        let (status, body) = fixture.post_empty("/push").await;
        assert_eq!((status, &body["status"]), (200, &json!("pushed")), "{body}");
        assert_eq!(
            plain_git(&bare, &["rev-parse", "main"]),
            plain_git(fixture.root(), &["rev-parse", "HEAD"])
        );
    }

    #[tokio::test]
    async fn a_fetch_says_how_far_apart_the_branches_are() {
        if !git_ready().await {
            return;
        }
        let fixture = Fixture::new().await;
        let bare = fixture.with_remote().await;
        let other = fixture.other(&bare);
        commit_elsewhere(&other, "b.txt", "two\n", "Add b");

        let (status, body) = fixture.post_empty("/fetch").await;
        assert_eq!(status, 200, "{body}");
        assert_eq!(
            body,
            json!({"upstream": "origin/main", "ahead": 0, "behind": 1})
        );
        // The fetch did not touch the working tree.
        assert!(!fixture.root().join("b.txt").exists());

        fixture.write("c.txt", "three\n");
        fixture.commit_all("Add c").await;
        let (_, body) = fixture.post_empty("/fetch").await;
        assert_eq!(
            body,
            json!({"upstream": "origin/main", "ahead": 1, "behind": 1})
        );
        let status = fixture.status().await;
        assert_eq!(
            (&status["ahead"], &status["behind"]),
            (&json!(1), &json!(1))
        );
    }

    #[tokio::test]
    async fn a_pull_fast_forwards_and_then_finds_nothing_to_do() {
        if !git_ready().await {
            return;
        }
        let fixture = Fixture::new().await;
        let bare = fixture.with_remote().await;
        let other = fixture.other(&bare);
        commit_elsewhere(&other, "a.txt", "one\ntwo\nthree\nfour\n", "Add a line");

        let (status, body) = fixture.post_empty("/pull").await;
        assert_eq!(status, 200, "{body}");
        assert_eq!(body, json!({"status": "fast-forward", "conflicts": []}));
        assert_eq!(fixture.read("a.txt"), "one\ntwo\nthree\nfour\n");

        let (status, body) = fixture.post_empty("/pull").await;
        assert_eq!(status, 200, "{body}");
        assert_eq!(body, json!({"status": "up-to-date", "conflicts": []}));
    }

    #[tokio::test]
    async fn a_pull_merges_changes_to_different_files() {
        if !git_ready().await {
            return;
        }
        let fixture = Fixture::new().await;
        let bare = fixture.with_remote().await;
        let other = fixture.other(&bare);
        commit_elsewhere(&other, "b.txt", "two\n", "Add b");
        fixture.write("c.txt", "three\n");
        fixture.commit_all("Add c").await;

        let (status, body) = fixture.post_empty("/pull").await;
        assert_eq!(status, 200, "{body}");
        assert_eq!(body, json!({"status": "merged", "conflicts": []}));
        assert_eq!(fixture.read("b.txt"), "two\n");
        assert_eq!(fixture.read("c.txt"), "three\n");
        assert_eq!(
            plain_git(fixture.root(), &["rev-list", "--merges", "--count", "HEAD"]),
            "1"
        );
        let status = fixture.status().await;
        assert_eq!(status["changes"], json!([]));
        assert_eq!(status["merging"], false);
    }

    #[tokio::test]
    async fn a_conflicting_pull_leaves_the_merge_to_the_editor_and_a_commit() {
        if !git_ready().await {
            return;
        }
        let fixture = Fixture::new().await;
        let bare = fixture.with_remote().await;
        let other = fixture.other(&bare);
        commit_elsewhere(&other, "a.txt", "one\ntheirs\nthree\n", "Their line");
        fixture.write("a.txt", "one\nmine\nthree\n");
        fixture.write("b.txt", "unrelated\n");
        fixture.commit_all("My line").await;

        let (status, body) = fixture.post_empty("/pull").await;
        assert_eq!(status, 200, "{body}");
        assert_eq!(body, json!({"status": "conflicts", "conflicts": ["a.txt"]}));
        assert!(fixture.read("a.txt").contains("<<<<<<<"));

        let status = fixture.status().await;
        assert_eq!(status["merging"], true);
        assert_eq!(
            status["changes"],
            json!([{"path": "a.txt", "kind": "conflicted"}])
        );
        // The merge has to be finished before another pull, a push of the
        // unfinished state or a partial commit.
        let (code, body) = fixture.post_empty("/pull").await;
        assert_eq!((code, &body["reason"]), (409, &json!("merging")), "{body}");
        let partial = json!({"message": "x", "paths": ["b.txt"]});
        let (code, body) = fixture.post("/commit", partial).await;
        assert_eq!((code, &body["reason"]), (409, &json!("merging")), "{body}");

        fixture.write("a.txt", "one\nboth\nthree\n");
        let (code, body) = fixture.post("/commit", json!({"message": "Merge"})).await;
        assert_eq!(code, 200, "{body}");
        let status = fixture.status().await;
        assert_eq!(status["merging"], false);
        assert_eq!(status["changes"], json!([]));
        assert_eq!(status["ahead"], 2);
        assert_eq!(fixture.read("a.txt"), "one\nboth\nthree\n");
        let (code, body) = fixture.post_empty("/push").await;
        assert_eq!((code, &body["status"]), (200, &json!("pushed")), "{body}");
    }

    #[tokio::test]
    async fn a_pull_waits_for_tracked_project_files_to_be_committed() {
        if !git_ready().await {
            return;
        }
        let fixture = Fixture::new().await;
        fixture.with_remote().await;
        fixture.write(".hidden", "x\n");
        fixture.write("out.pdf", "v1\n");
        fixture.commit_all("More files").await;
        plain_git(fixture.root(), &["push", "origin", "main"]);

        fixture.write("a.txt", "changed\n");
        fixture.write("untracked.txt", "new\n");
        let (status, body) = fixture.post_empty("/pull").await;
        assert_eq!(status, 409, "{body}");
        assert_eq!(body["reason"], "dirty");
        assert_eq!(body["paths"], json!(["a.txt"]));

        // A change Git sees but the project does not hold is not in the way,
        // and neither is a file Git does not track.
        plain_git(fixture.root(), &["checkout", "--", "a.txt"]);
        fixture.write(".hidden", "changed\n");
        fixture.write("out.pdf", "v2\n");
        fixture
            .inner
            .projects
            .note_render_outputs(&fixture.binding.id, ["out.pdf".to_string()]);
        let (status, body) = fixture.post_empty("/pull").await;
        assert_eq!(status, 200, "{body}");
        assert_eq!(body["status"], "up-to-date");
    }

    #[tokio::test]
    async fn a_branch_with_no_upstream_pulls_the_remote_branch_of_its_name() {
        if !git_ready().await {
            return;
        }
        let fixture = Fixture::new().await;
        let bare = fixture.with_remote().await;
        let other = fixture.other(&bare);
        commit_elsewhere(&other, "b.txt", "two\n", "Add b");
        plain_git(fixture.root(), &["branch", "--unset-upstream"]);
        assert!(fixture.status().await.get("upstream").is_none());

        let (status, body) = fixture.post_empty("/pull").await;
        assert_eq!(status, 200, "{body}");
        assert_eq!(body["status"], "fast-forward");
        assert_eq!(fixture.read("b.txt"), "two\n");

        // A branch the remote does not have has nothing to pull.
        fixture.post("/branch", json!({"branch": "topic"})).await;
        let (status, body) = fixture.post_empty("/pull").await;
        assert_eq!(status, 200, "{body}");
        assert_eq!(body["status"], "up-to-date");
    }

    #[tokio::test]
    async fn sync_commits_pulls_and_pushes_in_that_order() {
        if !git_ready().await {
            return;
        }
        let fixture = Fixture::new().await;
        let bare = fixture.with_remote().await;

        // Nothing to commit: two steps.
        let (status, body) = fixture.post("/sync", json!({})).await;
        assert_eq!(status, 200, "{body}");
        assert_eq!(
            body,
            json!({"steps": [
                {"step": "pull", "ok": true, "status": "up-to-date"},
                {"step": "push", "ok": true, "status": "up-to-date"},
            ]})
        );

        // A dirty tree, with a message of its own.
        fixture.write("b.txt", "two\n");
        let (status, body) = fixture.post("/sync", json!({"message": "Mine"})).await;
        assert_eq!(status, 200, "{body}");
        let steps = body["steps"].as_array().unwrap();
        let names: Vec<(&str, bool)> = steps
            .iter()
            .map(|step| {
                (
                    step["step"].as_str().unwrap(),
                    step["ok"].as_bool().unwrap(),
                )
            })
            .collect();
        assert_eq!(
            names,
            vec![("commit", true), ("pull", true), ("push", true)]
        );
        let commit = steps[0]["commit"].as_str().unwrap();
        assert_eq!(commit.len(), 40, "{commit}");
        assert!(commit.chars().all(|c| c.is_ascii_hexdigit()), "{commit}");
        assert_eq!(commit, plain_git(fixture.root(), &["rev-parse", "HEAD"]));
        assert_eq!(steps[1]["status"], "up-to-date");
        assert_eq!(steps[2]["status"], "pushed");
        assert!(steps[1].get("detail").is_none());
        assert!(steps[2].get("detail").is_none());
        assert_eq!(
            plain_git(&bare, &["log", "-1", "--format=%s", "main"]),
            "Mine"
        );

        // The default message, and a request with no body at all.
        fixture.write("c.txt", "three\n");
        let (status, body) = fixture.post_empty("/sync").await;
        assert_eq!(status, 200, "{body}");
        assert_eq!(body["steps"].as_array().unwrap().len(), 3);
        assert_eq!(
            plain_git(&bare, &["log", "-1", "--format=%s", "main"]),
            "Edits from LibrePaper"
        );
    }

    #[tokio::test]
    async fn sync_stops_at_the_first_step_that_fails() {
        if !git_ready().await {
            return;
        }
        // No identity: the commit fails and nothing runs after it.
        let fixture = Fixture::new().await;
        fixture.post_empty("/init").await;
        fixture.write("a.txt", "one\n");
        let (status, body) = fixture.post("/sync", json!({})).await;
        assert_eq!(status, 200, "{body}");
        let steps = body["steps"].as_array().unwrap();
        assert_eq!(steps.len(), 1);
        assert_eq!(steps[0]["step"], "commit");
        assert_eq!(steps[0]["ok"], false);
        assert_eq!(steps[0]["reason"], "identity");
        assert!(steps[0]["detail"].is_string());

        // A conflicting pull: no push.
        let fixture = Fixture::new().await;
        let bare = fixture.with_remote().await;
        let other = fixture.other(&bare);
        commit_elsewhere(&other, "a.txt", "one\ntheirs\nthree\n", "Their line");
        fixture.write("a.txt", "one\nmine\nthree\n");
        let (status, body) = fixture.post("/sync", json!({})).await;
        assert_eq!(status, 200, "{body}");
        let steps = body["steps"].as_array().unwrap();
        let names: Vec<(&str, bool)> = steps
            .iter()
            .map(|step| {
                (
                    step["step"].as_str().unwrap(),
                    step["ok"].as_bool().unwrap(),
                )
            })
            .collect();
        assert_eq!(names, vec![("commit", true), ("pull", false)]);
        assert_eq!(steps[1]["reason"], "conflicts");
        assert_eq!(steps[1]["status"], "conflicts");
        assert_eq!(steps[1]["conflicts"], json!(["a.txt"]));
        assert!(steps[1]["detail"].as_str().unwrap().contains("1 file"));
        assert_eq!(fixture.status().await["merging"], true);

        // No remote: the push fails after the commit and the pull.
        let fixture = Fixture::new().await;
        fixture.init().await;
        fixture.write("a.txt", "one\n");
        let (_, body) = fixture.post("/sync", json!({})).await;
        let steps = body["steps"].as_array().unwrap();
        assert_eq!(steps.len(), 2);
        assert_eq!(
            (steps[1]["step"].as_str(), steps[1]["reason"].as_str()),
            (Some("pull"), Some("no-remote"))
        );
    }

    #[tokio::test]
    async fn a_diff_sets_the_last_commit_against_the_working_tree() {
        if !git_ready().await {
            return;
        }
        let fixture = Fixture::new().await;
        fixture.init().await;
        fixture.write("a.txt", "one\n");
        fixture.write("gone.txt", "bye\n");
        fixture.commit_all("First").await;
        fixture.write("a.txt", "two\n");
        fixture.write("new.txt", "fresh\n");
        std::fs::remove_file(fixture.root().join("gone.txt")).unwrap();
        std::fs::write(fixture.root().join("blob.bin"), [0u8, 1, 2, 255]).unwrap();

        for (path, head, work) in [
            ("a.txt", json!("one\n"), json!("two\n")),
            ("new.txt", Value::Null, json!("fresh\n")),
            ("gone.txt", json!("bye\n"), Value::Null),
        ] {
            let (status, body) = fixture.get(&format!("/diff?path={path}")).await;
            assert_eq!(status, 200, "{path} {body}");
            assert_eq!(
                body,
                json!({"head": head, "work": work, "binary": false}),
                "{path}"
            );
        }
        let (status, body) = fixture.get("/diff?path=blob.bin").await;
        assert_eq!(status, 200, "{body}");
        assert_eq!(body, json!({"head": null, "work": null, "binary": true}));

        for path in [
            "/diff",
            "/diff?path=",
            "/diff?path=../x",
            "/diff?path=.git/config",
            "/diff?path=/etc/passwd",
        ] {
            assert_eq!(fixture.get(path).await.0, 400, "{path}");
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_diff_does_not_read_through_a_link() {
        if !git_ready().await {
            return;
        }
        let fixture = Fixture::new().await;
        fixture.init().await;
        fixture.write("a.txt", "one\n");
        fixture.commit_all("First").await;
        let outside = fixture.scratch.path().join("outside");
        std::fs::create_dir(&outside).unwrap();
        std::fs::write(outside.join("secret.txt"), "secret\n").unwrap();
        std::os::unix::fs::symlink(&outside, fixture.root().join("linked")).unwrap();
        std::os::unix::fs::symlink(outside.join("secret.txt"), fixture.root().join("peek.txt"))
            .unwrap();

        let (status, body) = fixture.get("/diff?path=linked/secret.txt").await;
        assert_eq!((status, &body["work"]), (200, &Value::Null), "{body}");
        let (status, body) = fixture.get("/diff?path=peek.txt").await;
        assert_eq!(status, 200, "{body}");
        assert_ne!(body["work"], "secret\n", "the target is named, not read");
    }

    #[tokio::test]
    async fn the_routes_belong_to_the_origin_that_opened_the_folder() {
        let fixture = Fixture::new().await;
        let tail = format!("projects/{}/git", fixture.binding.id);
        let (status, body) = fixture
            .send(Method::GET, &tail, "https://other.example", true, None)
            .await;
        assert_eq!(status, 404, "{body}");
        let (status, _) = fixture.send(Method::GET, &tail, PAGE, false, None).await;
        assert_eq!(status, 401);
        let (status, _) = fixture
            .send(Method::GET, "projects/q-nothing/git", PAGE, true, None)
            .await;
        assert_eq!(status, 404);
        for (method, path) in [
            (Method::POST, "/"),
            (Method::GET, "/commit"),
            (Method::GET, "/init"),
            (Method::POST, "/status"),
            (Method::DELETE, ""),
        ] {
            let (status, _) = fixture.route(method, path, None).await;
            assert_eq!(status, 404, "{path}");
        }
    }

    /// Makes a bare repository with one commit, and `refs/librepaper/state`
    /// pointing at it, as a remote that has been kept with LibrePaper.
    fn seeded_remote(scratch: &Path) -> PathBuf {
        let bare = scratch.join("paper.git");
        plain_git(scratch, &["init", "--bare", "-b", "main", "paper.git"]);
        let seed = scratch.join("seed");
        plain_git(scratch, &["clone", text(&bare), "seed"]);
        std::fs::write(seed.join("main.tex"), "hello\n").unwrap();
        plain_git(&seed, &["add", "-A"]);
        plain_git(&seed, &["commit", "-m", "First"]);
        plain_git(&seed, &["push", "origin", "main"]);
        plain_git(&seed, &["update-ref", "refs/librepaper/state", "HEAD"]);
        plain_git(&seed, &["push", "origin", "refs/librepaper/state"]);
        bare
    }

    #[tokio::test]
    async fn a_clone_becomes_a_folder_project_with_the_librepaper_state() {
        if !git_ready().await {
            return;
        }
        let fixture = Fixture::new().await;
        let bare = seeded_remote(fixture.scratch.path());
        let parent = fixture.scratch.path().join("projects");
        std::fs::create_dir(&parent).unwrap();

        let opened = run_clone(&fixture.inner, Some(PAGE), text(&bare), "paper", &parent)
            .await
            .unwrap();
        assert_eq!(opened["name"], "paper");
        assert_eq!(opened["root_name"], "paper");
        let id = opened["id"].as_str().unwrap();
        let binding = fixture.inner.bindings.get_project(id, PAGE).unwrap();
        assert_eq!(
            binding.root,
            std::fs::canonicalize(parent.join("paper")).unwrap()
        );
        assert_eq!(
            std::fs::read_to_string(parent.join("paper/main.tex")).unwrap(),
            "hello\n"
        );
        assert_eq!(
            plain_git(
                &parent.join("paper"),
                &["rev-parse", "refs/librepaper/state"]
            ),
            plain_git(&parent.join("paper"), &["rev-parse", "HEAD"])
        );
        assert_eq!(
            plain_git(
                &parent.join("paper"),
                &["rev-parse", "--abbrev-ref", "@{upstream}"]
            ),
            "origin/main"
        );

        // The folder is there now.
        let (status, body) = run_clone(&fixture.inner, Some(PAGE), text(&bare), "paper", &parent)
            .await
            .unwrap_err();
        assert_eq!((status, &body["reason"]), (409, &json!("exists")), "{body}");
    }

    #[tokio::test]
    async fn a_clone_that_fails_leaves_no_folder_and_no_project() {
        if !git_ready().await {
            return;
        }
        let fixture = Fixture::new().await;
        let parent = fixture.scratch.path().join("projects");
        std::fs::create_dir(&parent).unwrap();
        let missing = fixture.scratch.path().join("nothing.git");
        let (status, body) = run_clone(
            &fixture.inner,
            Some(PAGE),
            text(&missing),
            "nothing",
            &parent,
        )
        .await
        .unwrap_err();
        assert!(matches!(status, 422 | 502), "{status} {body}");
        assert!(!body["error"].as_str().unwrap().is_empty());
        assert!(!parent.join("nothing").exists());
        let projects = fixture.inner.bindings.list_projects(PAGE);
        assert_eq!(projects.len(), 1, "only the fixture's own folder");
    }

    #[tokio::test]
    async fn a_clone_refuses_urls_that_are_not_https_ssh_or_scp_without_a_password() {
        let fixture = Fixture::new().await;
        let headers = test_support::bearer(&fixture.inner);
        for url in [
            "http://github.com/o/r.git",
            "https://user:secret@github.com/o/r.git",
            "ssh://user:secret@github.com/o/r.git",
            "git://github.com/o/r.git",
            "file:///tmp/r.git",
            "/tmp/r.git",
            "C:\\r.git",
            "ext::sh -c id",
            "-oProxyCommand=x",
            "github.com:o/r.git",
            "https://github.com/o/.git",
            "",
        ] {
            let request = json_request(json!({"url": url}));
            let reply = handle_clone(&fixture.inner, &headers, Some(PAGE), request).await;
            let (status, body) = read(reply).await;
            assert_eq!(status, 400, "{url:?} {body}");
        }
        // Without a credential nothing is read.
        let request = json_request(json!({"url": "https://github.com/o/r.git"}));
        let reply = handle_clone(&fixture.inner, &HeaderMap::new(), Some(PAGE), request).await;
        assert_eq!(read(reply).await.0, 401);
    }

    #[test]
    fn a_failed_command_is_sorted_into_what_the_panel_acts_on() {
        let failed = |stderr: &str| Output {
            code: 128,
            stdout: Vec::new(),
            stderr: stderr.to_string(),
        };

        let (status, body) = remote_failure(&failed(
            "fatal: could not read Username for 'https://token@github.com': terminal prompts disabled\n",
        ));
        assert_eq!((status, &body["reason"]), (502, &json!("auth")));
        let detail = body["detail"].as_str().unwrap();
        assert!(detail.contains("terminal prompts disabled"), "{detail}");
        assert!(!detail.contains("token@"), "{detail}");
        assert_eq!(body["error"], body["detail"]);

        let (status, body) = remote_failure(&failed(
            "fatal: unable to access 'https://github.com/o/r.git/': Could not resolve host: github.com",
        ));
        assert_eq!((status, &body["reason"]), (502, &json!("network")));

        let (status, body) = remote_failure(&failed("remote: Repository not found."));
        assert_eq!((status, &body["reason"]), (502, &json!("not-found")));

        // Not the remote's doing: Git's own words, as a refusal.
        let (status, body) = remote_failure(&failed(
            "error: Your local changes to the following files would be overwritten by merge:\n\ta.txt",
        ));
        assert_eq!(status, 422);
        assert!(body["error"].as_str().unwrap().contains("a.txt"));
        assert!(body.get("reason").is_none());
    }

    #[test]
    fn a_missing_git_and_a_slow_remote_have_their_own_answers() {
        let (status, body) = run_failure(RunError::Missing, Limit::Local);
        assert_eq!((status, &body["reason"]), (409, &json!("git-missing")));
        let (status, body) = run_failure(RunError::TooOld("2.30.1".into()), Limit::Local);
        assert_eq!((status, &body["reason"]), (409, &json!("git-old")));
        assert!(body["error"].as_str().unwrap().contains("2.30.1"));
        let (status, body) = run_failure(RunError::Timeout, Limit::Transport);
        assert_eq!((status, &body["reason"]), (502, &json!("network")));
        assert_eq!(run_failure(RunError::Timeout, Limit::Local).0, 500);
        assert_eq!(run_failure(RunError::TooLarge, Limit::Local).0, 500);
    }

    #[test]
    fn only_the_files_the_project_holds_can_make_a_pull_wait() {
        let change = |path: &str, tracked: bool| status::Change {
            path: path.to_string(),
            kind: status::Kind::Modified,
            from: None,
            tracked,
        };
        let status = Status {
            changes: vec![
                change("main.tex", true),
                change("chapters/one.tex", true),
                change("notes.txt", false),
                change(".gitignore", true),
                change(".github/workflows/librepaper.yml", true),
                change("node_modules/pkg/index.js", true),
                change("main.pdf", true),
            ],
            ..Status::default()
        };
        let outputs: BTreeSet<String> = ["main.pdf".to_string()].into();
        assert_eq!(
            dirty_paths(&status, &outputs),
            vec!["main.tex".to_string(), "chapters/one.tex".to_string()]
        );
    }

    #[test]
    fn a_pathspec_list_matches_each_path_as_typed() {
        let list = pathspec_list(&["a [1].txt".to_string(), "*.tex".to_string()]);
        assert_eq!(list, b":(literal)a [1].txt\0:(literal)*.tex\0".to_vec());
        assert!(committable("dir/a.txt"));
        assert!(!committable(".git/config"));
        assert!(!committable("a/.GIT/x"));
        assert!(!committable("../x"));
    }
}
