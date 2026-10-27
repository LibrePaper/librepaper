//! Connections: a web project joined to a remote Git repository, with no
//! folder the person chose.
//!
//! The desktop app clones the repository into the state home, under
//! `connections/<key>/repo`, and keeps the link and the hashes of what was
//! last pulled or pushed in the clone's own refs, as a git folder does. The
//! person never sees the clone. Pull and push move the project's named versions
//! and the repository's commits through it, with the person's own Git
//! credentials, and a merge that stops with conflicts is resolved here one file
//! at a time.
//!
//! Only one git operation runs per connection at a time. The work is the same
//! as the git routes' and the sync crate's, called from `git.rs` and `sync.rs`.

use super::*;

use std::collections::BTreeSet;

use sha2::{Digest, Sha256};

use super::git::{
    conflicted_paths, declined, merge_in_progress, refuse_because, remote_failure, run, run_pull,
    run_push, SYNC_MESSAGE,
};
use super::sync::{
    answer, begin_login, client_for, failed, finish_login, parse_link, project_url, refuse,
    same_site, token_refused, Failure,
};

use crate::local::credentials;
use crate::local::git::{validate, Limit, Output};

use librepaper_base::http::Credentials;
use librepaper_sync::{
    commit_summary, current_branch, pull, push, store_for, web_changed, Client, Error, Link,
    MergeOutcome, PullOptions, PullOutcome, PushOptions, PushOutcome, State,
};

/// The folder of every connection, under the state home.
const CONNECTIONS: &str = "connections";
/// The clone, inside a connection's folder.
const REPOSITORY: &str = "repo";
/// The clone while it is being made. It is renamed to `REPOSITORY` on success.
const PARTIAL: &str = "repo.partial";
/// What the connection is for and where it clones from.
const RECORD: &str = "connection.json";
/// The identity merges are committed with, when the clone has none.
const MERGE_NAME: &str = "LibrePaper";
const MERGE_EMAIL: &str = "noreply@librepaper.org";

/// The connections with a git operation running. One operation at a time per
/// connection: a second one is refused with `busy` until the first finishes.
#[derive(Default)]
pub(super) struct Busy {
    keys: std::sync::Mutex<HashSet<String>>,
}

impl Busy {
    fn keys(&self) -> std::sync::MutexGuard<'_, HashSet<String>> {
        self.keys
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Claims `key` for one operation until the returned guard is dropped.
    fn claim(&self, key: &str) -> Result<Hold<'_>, Failure> {
        if self.keys().insert(key.to_owned()) {
            Ok(Hold {
                busy: self,
                key: key.to_owned(),
            })
        } else {
            Err(refuse_because(
                409,
                "busy",
                "another git operation is running for this project",
            ))
        }
    }
}

/// A claimed connection, released when dropped.
struct Hold<'a> {
    busy: &'a Busy,
    key: String,
}

impl Drop for Hold<'_> {
    fn drop(&mut self) {
        self.busy.keys().remove(&self.key);
    }
}

/// The connection to one project: its key, its link and its folder.
struct Connection {
    key: String,
    link: Link,
    dir: PathBuf,
}

impl Connection {
    fn open(inner: &Inner, link: Link) -> Connection {
        let key = connection_key(&link);
        let dir = inner.state_home.join(CONNECTIONS).join(&key);
        Connection { key, link, dir }
    }

    fn repo(&self) -> PathBuf {
        self.dir.join(REPOSITORY)
    }

    fn connected(&self) -> bool {
        self.repo().exists()
    }

    /// The owner a sign-in for this connection is recorded under.
    fn owner(&self) -> String {
        format!("connection:{}", self.key)
    }
}

/// The first 32 hex characters of the SHA-256 of the project's address.
fn connection_key(link: &Link) -> String {
    hex::encode(Sha256::digest(project_url(link).as_bytes()))[..32].to_string()
}

/// What `connection.json` holds: the project's address and the remote's.
#[derive(serde::Serialize, serde::Deserialize)]
struct Record {
    project: String,
    url: String,
}

fn read_record(dir: &Path) -> Result<Record, Failure> {
    let unreadable = |error: &dyn std::fmt::Display| {
        refuse(500, &format!("could not read the connection: {error}"))
    };
    let bytes = std::fs::read(dir.join(RECORD)).map_err(|error| unreadable(&error))?;
    serde_json::from_slice(&bytes).map_err(|error| unreadable(&error))
}

/// The connection of `project`, which must be a project on the page's site.
fn open_connection(
    inner: &Inner,
    origin: Option<&str>,
    project: &str,
) -> Result<Connection, Failure> {
    Ok(Connection::open(inner, parse_link(project, origin)?))
}

/// The clone of a connected project, or the refusal that it is not connected.
fn require_connected(connection: &Connection) -> Result<PathBuf, Failure> {
    if connection.connected() {
        Ok(connection.repo())
    } else {
        Err(refuse_because(
            409,
            "not-connected",
            "this project is not connected to a repository",
        ))
    }
}

/// Runs Git in `root`, and answers Git's refusal as one.
async fn must(root: &Path, limit: Limit, args: &[&str]) -> Result<Output, Failure> {
    let output = run(root, limit, args).await?;
    if output.success() {
        Ok(output)
    } else {
        Err(declined(&output))
    }
}

/// Which side the merge in progress brought in: `project` when MERGE_HEAD is
/// the web project's version on `refs/librepaper/main`, else `repository`.
async fn merge_side(repo: &Path) -> Result<&'static str, Failure> {
    let incoming = run(
        repo,
        Limit::Local,
        &["rev-parse", "-q", "--verify", "MERGE_HEAD"],
    )
    .await?
    .text();
    let project = run(
        repo,
        Limit::Local,
        &["rev-parse", "-q", "--verify", "refs/librepaper/main"],
    )
    .await?
    .text();
    Ok(if !project.is_empty() && incoming == project {
        "project"
    } else {
        "repository"
    })
}

/// Whether the token this computer holds for the project's site still works.
/// A connected project asks the site, as a folder project's status does, so a
/// revoked or expired token reads as signed out. Any other failure to reach
/// the site leaves the answer to the pull or push that meets it.
async fn token_works(inner: &Inner, connection: &Connection, token: &str) -> bool {
    if token.is_empty() {
        return false;
    }
    if !connection.connected() {
        return true;
    }
    let store = store_for(&connection.repo(), &inner.state_home);
    let Ok(Some(state)) = store.load() else {
        return true;
    };
    let client = Client::new(connection.link.site.clone(), Credentials::token(token));
    !matches!(web_changed(&client, store.as_ref(), &state).await, Err(error) if token_refused(&error))
}

/// Whether a merge in the clone still needs the page. A merge whose files are
/// all settled but whose commit failed, to a hook or a signing key, is
/// committed again here, so Pull or Push is the way to retry it. A commit that
/// fails again says why.
async fn unfinished_merge(repo: &Path) -> Result<bool, Failure> {
    if !merge_in_progress(repo).await? {
        return Ok(false);
    }
    if !conflicted_paths(repo).await?.is_empty() {
        return Ok(true);
    }
    must(repo, Limit::Local, &["commit", "--no-edit"]).await?;
    Ok(false)
}

/// The status object every route answers with.
async fn status_of(inner: &Inner, connection: &Connection) -> Result<Value, Failure> {
    let token = credentials::stored_token_at(&inner.state_home, connection.link.site.as_str());
    let signed_in = token_works(inner, connection, &token).await;
    if !connection.connected() {
        return Ok(json!({"connected": false, "signed_in": signed_in}));
    }
    let repo = connection.repo();
    let record = read_record(&connection.dir)?;
    let branch = run(
        &repo,
        Limit::Local,
        &["symbolic-ref", "--short", "-q", "HEAD"],
    )
    .await?;
    let branch = if branch.success() {
        json!(branch.text())
    } else {
        Value::Null
    };
    let merging = if merge_in_progress(&repo).await? {
        json!({
            "side": merge_side(&repo).await?,
            "conflicts": conflicted_paths(&repo).await?,
        })
    } else {
        Value::Null
    };
    Ok(json!({
        "connected": true,
        "signed_in": signed_in,
        "url": validate::display_url(&record.url),
        "branch": branch,
        "merging": merging,
    }))
}

/// The answer when a merge stops with conflicts for the page to resolve.
async fn stopped(inner: &Inner, connection: &Connection) -> Result<Value, Failure> {
    Ok(json!({"stopped": "conflicts", "status": status_of(inner, connection).await?}))
}

/// The refusal while a merge is open in the clone, with the status.
async fn merging(inner: &Inner, connection: &Connection) -> Failure {
    match status_of(inner, connection).await {
        Ok(mut body) => {
            body["reason"] = json!("merging");
            (409, body)
        }
        Err(failure) => failure,
    }
}

/// Gives the clone an identity for the merges it commits, when it has none.
async fn ensure_identity(repo: &Path) -> Result<(), Failure> {
    let email = run(repo, Limit::Local, &["config", "--get", "user.email"])
        .await?
        .text();
    if email.is_empty() {
        must(repo, Limit::Local, &["config", "user.name", MERGE_NAME]).await?;
        must(repo, Limit::Local, &["config", "user.email", MERGE_EMAIL]).await?;
    }
    Ok(())
}

/* ------------------------------------------------------------ status */

/// The status of the connection of the `project` named in the query.
pub(super) async fn handle_status(
    inner: &Arc<Inner>,
    headers: &HeaderMap,
    origin: Option<&str>,
    request: Request<Body>,
) -> Reply {
    if let Err(response) = authenticate(inner, headers, origin) {
        return response;
    }
    let Some(project) = query_value(request.uri(), "project") else {
        return write_json(400, &json!({"error": "a connection needs a project"}));
    };
    let status = async {
        let connection = open_connection(inner, origin, &project)?;
        status_of(inner, &connection).await
    };
    answer(status.await)
}

/* ----------------------------------------------------------- connect */

#[derive(serde::Deserialize)]
struct ConnectBody {
    project: String,
    url: String,
}

pub(super) async fn handle_connect(
    inner: &Arc<Inner>,
    headers: &HeaderMap,
    origin: Option<&str>,
    request: Request<Body>,
) -> Reply {
    if let Err(response) = authenticate(inner, headers, origin) {
        return response;
    }
    let body = match read_json_body::<ConnectBody>(request).await {
        Ok(body) => body,
        Err(response) => return response,
    };
    let url = match validate::validate_url(&body.url) {
        Ok(url) => url,
        Err(why) => return write_json(400, &json!({"error": why})),
    };
    let connection = match open_connection(inner, origin, &body.project) {
        Ok(connection) => connection,
        Err((status, failure)) => return write_json(status, &failure),
    };
    answer(run_connect(inner, &connection, &url).await)
}

/// Clones `url` into the connection and keeps the project's state in the
/// clone, then answers the status. A connection that fails leaves no folder.
async fn run_connect(inner: &Inner, connection: &Connection, url: &str) -> Result<Value, Failure> {
    let _hold = inner.connections.claim(&connection.key)?;
    if connection.connected() {
        return Err(refuse_because(
            409,
            "connected",
            "this project is already connected to a repository",
        ));
    }
    let made = clone_for(inner, connection, url).await;
    if made.is_err() {
        let _ = std::fs::remove_dir_all(&connection.dir);
    }
    made?;
    status_of(inner, connection).await
}

/// The clone behind `run_connect`. The clone is made under `PARTIAL` and
/// renamed last, once the state and the record are in place.
async fn clone_for(inner: &Inner, connection: &Connection, url: &str) -> Result<(), Failure> {
    let unwritable =
        |error: std::io::Error| refuse(500, &format!("could not make the connection: {error}"));
    std::fs::create_dir_all(&connection.dir).map_err(unwritable)?;
    let partial = connection.dir.join(PARTIAL);
    let _ = std::fs::remove_dir_all(&partial);
    let cloned = run(
        &connection.dir,
        Limit::Transport,
        &["clone", "--", url, PARTIAL],
    )
    .await?;
    if !cloned.success() {
        return Err(remote_failure(&cloned));
    }
    // The state is there only when the repository was kept with LibrePaper
    // before. A fetch that matches nothing succeeds, so a failure here is the
    // network or the credential, and the state it would have read could link
    // the repository to another project.
    let fetched = run(
        &partial,
        Limit::Transport,
        &["fetch", "--", url, "+refs/librepaper/*:refs/librepaper/*"],
    )
    .await?;
    if !fetched.success() {
        return Err(remote_failure(&fetched));
    }

    let store = store_for(&partial, &inner.state_home);
    match store.load().map_err(|error| failed(&error))? {
        Some(state)
            if !(same_site(&state.link.site, &connection.link.site)
                && state.link.slug == connection.link.slug) =>
        {
            return Err(refuse_because(
                409,
                "linked",
                "this repository is linked to another LibrePaper project",
            ));
        }
        Some(_) => {}
        None => store
            .save(&State::new(connection.link.clone()))
            .map_err(|error| failed(&error))?,
    }
    ensure_identity(&partial).await?;

    let record = Record {
        project: project_url(&connection.link),
        url: url.to_owned(),
    };
    let bytes =
        serde_json::to_vec_pretty(&record).map_err(|error| refuse(500, &error.to_string()))?;
    std::fs::write(connection.dir.join(RECORD), bytes).map_err(unwritable)?;
    std::fs::rename(&partial, connection.repo()).map_err(unwritable)?;
    Ok(())
}

/* ----------------------------------------------------- pull and push */

/// The remote's commits taken into the clone's branch, as `git pull` takes
/// them. A remote that does not have the branch yet, such as a new empty
/// repository, has nothing to give, so the first push can make the branch.
/// The fetch runs first so that a remote out of reach still says so.
async fn pull_remote(repo: &Path) -> Result<Value, Failure> {
    let fetched = run(repo, Limit::Transport, &["fetch", "origin"]).await?;
    if !fetched.success() {
        return Err(remote_failure(&fetched));
    }
    let upstream = run(
        repo,
        Limit::Local,
        &["rev-parse", "-q", "--verify", "@{upstream}"],
    )
    .await?;
    if !upstream.success() {
        return Ok(json!({"status": "up-to-date", "conflicts": []}));
    }
    run_pull(repo, &BTreeSet::new()).await
}

/// The project's named versions taken into the clone, merged into its branch.
async fn web_pull(inner: &Inner, connection: &Connection) -> Result<PullOutcome, Failure> {
    let client = Arc::new(client_for(inner, &connection.link.site)?);
    let (root, state_home) = (connection.repo(), inner.state_home.clone());
    let handle = tokio::runtime::Handle::current();
    let outcome = tokio::task::spawn_blocking(move || {
        let store = store_for(&root, &state_home);
        // The clone is the app's own, so the branch it has is the branch it
        // syncs.
        let options = PullOptions {
            merge: true,
            mine: Vec::new(),
            theirs: Vec::new(),
            exclude: Vec::new(),
            branch: Some(current_branch(&root)?),
        };
        handle.block_on(pull(&client, &root, store.as_ref(), &options))
    })
    .await
    .map_err(|_| refuse(500, "the pull stopped"))?
    .map_err(|error| failed(&error))?;
    Ok(outcome)
}

/// Whether a web pull left a merge to resolve. A clone holding changes the
/// merge cannot be made over is refused.
fn merge_stopped(outcome: &PullOutcome) -> Result<bool, Failure> {
    match outcome {
        PullOutcome::Repository {
            merge: MergeOutcome::Conflicts(_),
            ..
        } => Ok(true),
        PullOutcome::Repository {
            merge: MergeOutcome::DirtyTree(paths),
            ..
        } => Err((
            409,
            json!({
                "reason": "dirty",
                "paths": paths,
                "error": "Commit your changes to tracked files first.",
            }),
        )),
        _ => Ok(false),
    }
}

/// The project's named versions sent from the clone as one version. The
/// message and author are those of the clone's latest commit, as the command
/// line makes them.
async fn web_push(inner: &Inner, connection: &Connection) -> Result<PushOutcome, Failure> {
    let client = Arc::new(client_for(inner, &connection.link.site)?);
    let (root, state_home) = (connection.repo(), inner.state_home.clone());
    let handle = tokio::runtime::Handle::current();
    let outcome = tokio::task::spawn_blocking(move || -> Result<PushOutcome, Error> {
        let store = store_for(&root, &state_home);
        let state = store.load()?.ok_or(Error::NotLinked)?;
        let (message, author) = match commit_summary(&root, &state) {
            Some(summary) => (summary.message, Some(summary.author)),
            None => (SYNC_MESSAGE.to_owned(), None),
        };
        let options = PushOptions {
            message,
            author,
            exclude: Vec::new(),
            branch: Some(current_branch(&root)?),
        };
        handle.block_on(push(&client, &root, store.as_ref(), &options))
    })
    .await
    .map_err(|_| refuse(500, "the push stopped"))?
    .map_err(|error| failed(&error))?;
    Ok(outcome)
}

/// The answer for a web push that did not send a version.
fn pushed_refusal(outcome: &PushOutcome) -> Option<Failure> {
    match outcome {
        PushOutcome::Refused { paths } => Some((409, json!({"reason": "changed", "paths": paths}))),
        PushOutcome::Suggestion { path } => {
            Some((409, json!({"reason": "suggestion", "path": path})))
        }
        PushOutcome::Nothing | PushOutcome::Pushed { .. } => None,
    }
}

#[derive(serde::Deserialize)]
struct ProjectOnly {
    project: String,
}

/// Brings the repository's commits in, then the project's versions into the
/// branch, then sends the branch's changes as one version. Stops at the first
/// conflict.
pub(super) async fn handle_pull(
    inner: &Arc<Inner>,
    headers: &HeaderMap,
    origin: Option<&str>,
    request: Request<Body>,
) -> Reply {
    if let Err(response) = authenticate(inner, headers, origin) {
        return response;
    }
    let body = match read_json_body::<ProjectOnly>(request).await {
        Ok(body) => body,
        Err(response) => return response,
    };
    answer(pull_from_repository(inner, origin, &body.project).await)
}

async fn pull_from_repository(
    inner: &Inner,
    origin: Option<&str>,
    project: &str,
) -> Result<Value, Failure> {
    let connection = open_connection(inner, origin, project)?;
    let _hold = inner.connections.claim(&connection.key)?;
    let repo = require_connected(&connection)?;
    if unfinished_merge(&repo).await? {
        return Err(merging(inner, &connection).await);
    }
    let fetched = pull_remote(&repo).await?;
    if fetched["status"] == "conflicts" {
        return stopped(inner, &connection).await;
    }
    if merge_stopped(&web_pull(inner, &connection).await?)? {
        return stopped(inner, &connection).await;
    }
    let outcome = web_push(inner, &connection).await?;
    if let Some(refusal) = pushed_refusal(&outcome) {
        return Err(refusal);
    }
    let pushed = match outcome {
        PushOutcome::Pushed { files, .. } => json!({"files": files}),
        _ => json!({"nothing": true}),
    };
    Ok(json!({"status": status_of(inner, &connection).await?, "pushed": pushed}))
}

/// Brings the project's versions into the repository: the web versions are
/// merged into the branch, the repository's commits are fetched and merged, and
/// the branch is pushed. Stops at the first conflict.
pub(super) async fn handle_push(
    inner: &Arc<Inner>,
    headers: &HeaderMap,
    origin: Option<&str>,
    request: Request<Body>,
) -> Reply {
    if let Err(response) = authenticate(inner, headers, origin) {
        return response;
    }
    let body = match read_json_body::<ProjectOnly>(request).await {
        Ok(body) => body,
        Err(response) => return response,
    };
    answer(push_to_repository(inner, origin, &body.project).await)
}

async fn push_to_repository(
    inner: &Inner,
    origin: Option<&str>,
    project: &str,
) -> Result<Value, Failure> {
    let connection = open_connection(inner, origin, project)?;
    let _hold = inner.connections.claim(&connection.key)?;
    let repo = require_connected(&connection)?;
    if unfinished_merge(&repo).await? {
        return Err(merging(inner, &connection).await);
    }
    if merge_stopped(&web_pull(inner, &connection).await?)? {
        return stopped(inner, &connection).await;
    }
    let fetched = pull_remote(&repo).await?;
    if fetched["status"] == "conflicts" {
        return stopped(inner, &connection).await;
    }
    let pushed = run_push(&repo).await?;
    Ok(json!({
        "status": status_of(inner, &connection).await?,
        "pushed": pushed["status"].clone(),
    }))
}

/* ----------------------------------------------------- resolve a merge */

#[derive(serde::Deserialize)]
struct ResolveBody {
    project: String,
    path: String,
    keep: String,
}

/// Keeps one side of a conflicted file, `keep` naming `project` or
/// `repository`, and commits the merge once no conflict is left.
pub(super) async fn handle_resolve(
    inner: &Arc<Inner>,
    headers: &HeaderMap,
    origin: Option<&str>,
    request: Request<Body>,
) -> Reply {
    if let Err(response) = authenticate(inner, headers, origin) {
        return response;
    }
    let body = match read_json_body::<ResolveBody>(request).await {
        Ok(body) => body,
        Err(response) => return response,
    };
    answer(resolve(inner, origin, &body).await)
}

async fn resolve(
    inner: &Inner,
    origin: Option<&str>,
    body: &ResolveBody,
) -> Result<Value, Failure> {
    let connection = open_connection(inner, origin, &body.project)?;
    let _hold = inner.connections.claim(&connection.key)?;
    let repo = require_connected(&connection)?;
    if !merge_in_progress(&repo).await? {
        return Err(refuse_because(
            409,
            "not-merging",
            "no merge is in progress",
        ));
    }
    if body.keep != "project" && body.keep != "repository" {
        return Err(refuse(
            400,
            "keep names the side to keep: project or repository",
        ));
    }
    if !conflicted_paths(&repo).await?.contains(&body.path) {
        return Err(refuse(400, "that file is not in conflict"));
    }
    let theirs = body.keep == merge_side(&repo).await?;
    let (flag, stage) = if theirs {
        ("--theirs", 3)
    } else {
        ("--ours", 2)
    };
    // The side that deleted the file has no entry at its stage.
    let present = run(
        &repo,
        Limit::Local,
        &["cat-file", "-e", &format!(":{stage}:{}", body.path)],
    )
    .await?
    .success();
    if present {
        must(&repo, Limit::Local, &["checkout", flag, "--", &body.path]).await?;
        must(&repo, Limit::Local, &["add", "--", &body.path]).await?;
    } else {
        must(&repo, Limit::Local, &["rm", "-q", "--", &body.path]).await?;
    }
    if conflicted_paths(&repo).await?.is_empty() {
        must(&repo, Limit::Local, &["commit", "--no-edit"]).await?;
    }
    status_of(inner, &connection).await
}

/* ------------------------------------------------------------ disconnect */

/// Removes the connection's folder, and with it the clone and its state.
pub(super) async fn handle_disconnect(
    inner: &Arc<Inner>,
    headers: &HeaderMap,
    origin: Option<&str>,
    request: Request<Body>,
) -> Reply {
    if let Err(response) = authenticate(inner, headers, origin) {
        return response;
    }
    let body = match read_json_body::<ProjectOnly>(request).await {
        Ok(body) => body,
        Err(response) => return response,
    };
    answer(disconnect(inner, origin, &body.project).await)
}

async fn disconnect(inner: &Inner, origin: Option<&str>, project: &str) -> Result<Value, Failure> {
    let connection = open_connection(inner, origin, project)?;
    let _hold = inner.connections.claim(&connection.key)?;
    match std::fs::remove_dir_all(&connection.dir) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => {
            return Err(refuse(
                500,
                &format!("could not remove the connection: {error}"),
            ))
        }
    }
    status_of(inner, &connection).await
}

/* ----------------------------------------------------------- sign in */

/// Asks the project's site for a code, for the connection's own sign-in.
pub(super) async fn handle_login(
    inner: &Arc<Inner>,
    headers: &HeaderMap,
    origin: Option<&str>,
    request: Request<Body>,
) -> Reply {
    if let Err(response) = authenticate(inner, headers, origin) {
        return response;
    }
    let body = match read_json_body::<ProjectOnly>(request).await {
        Ok(body) => body,
        Err(response) => return response,
    };
    let begun = async {
        let connection = open_connection(inner, origin, &body.project)?;
        begin_login(
            inner,
            origin,
            connection.link.site.clone(),
            &connection.owner(),
        )
        .await
    };
    answer(begun.await)
}

#[derive(serde::Deserialize)]
struct LoginCompleteBody {
    project: String,
    authorization_id: String,
}

/// Claims the token once the code is approved, and answers the status.
pub(super) async fn handle_login_complete(
    inner: &Arc<Inner>,
    headers: &HeaderMap,
    origin: Option<&str>,
    request: Request<Body>,
) -> Reply {
    if let Err(response) = authenticate(inner, headers, origin) {
        return response;
    }
    let body = match read_json_body::<LoginCompleteBody>(request).await {
        Ok(body) => body,
        Err(response) => return response,
    };
    let finished = async {
        let connection = open_connection(inner, origin, &body.project)?;
        finish_login(inner, origin, &connection.owner(), &body.authorization_id).await?;
        status_of(inner, &connection).await
    };
    answer(finished.await)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::local::git;

    const PAGE: &str = "https://paper.example";
    const PROJECT: &str = "https://paper.example/docs/abc123";

    /// Whether this machine has a Git the routes accept. The tests that run
    /// Git return at once without one.
    async fn git_ready() -> bool {
        matches!(git::locate().await, git::Found::Ready { .. })
    }

    /// Plain Git for test setup, away from the configuration of whoever runs
    /// the tests, with an identity that is not the project's. Answers the
    /// whole output, whatever the exit status.
    fn plain_git_output(dir: &Path, args: &[&str]) -> std::process::Output {
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
        command.output().expect("git runs")
    }

    /// Plain Git for test setup, which must succeed. Answers stdout.
    fn plain_git(dir: &Path, args: &[&str]) -> String {
        let output = plain_git_output(dir, args);
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

    /// A bare repository whose `main` holds `main.tex`: the remote a
    /// connection clones.
    fn bare_remote(scratch: &Path) -> PathBuf {
        let bare = scratch.join("paper.git");
        plain_git(scratch, &["init", "--bare", "-b", "main", "paper.git"]);
        let seed = scratch.join("seed");
        plain_git(scratch, &["clone", text(&bare), "seed"]);
        std::fs::write(seed.join("main.tex"), "hello\n").unwrap();
        plain_git(&seed, &["add", "-A"]);
        plain_git(&seed, &["commit", "-m", "First"]);
        plain_git(&seed, &["push", "origin", "main"]);
        bare
    }

    struct Fixture {
        inner: Arc<Inner>,
        scratch: tempfile::TempDir,
        _state: tempfile::TempDir,
    }

    impl Fixture {
        async fn new() -> Fixture {
            let state = tempfile::tempdir().unwrap();
            let inner = test_support::inner(state.path(), state.path()).await;
            Fixture {
                inner,
                scratch: tempfile::tempdir().unwrap(),
                _state: state,
            }
        }

        /// The connection of `PROJECT` on the page `PAGE`.
        fn connection(&self, project: &str) -> Connection {
            let link = parse_link(project, Some(PAGE)).unwrap();
            Connection::open(&self.inner, link)
        }

        /// One request through the router, as the desktop shell sends it.
        async fn send(&self, method: Method, tail: &str, body: Option<Value>) -> (u16, Value) {
            let builder = Request::builder()
                .method(method)
                .uri(format!("{BASE_PATH}/{tail}"))
                .header("host", format!("127.0.0.1:{}", test_support::PORT))
                .header("x-librepaper-origin", PAGE)
                .header(
                    "authorization",
                    format!("Bearer {}", test_support::credential(&self.inner)),
                );
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

    #[tokio::test]
    async fn a_connection_lives_in_a_folder_named_by_its_project_address() {
        let fixture = Fixture::new().await;
        let connection = fixture.connection(PROJECT);
        let key =
            hex::encode(Sha256::digest(b"https://paper.example/docs/abc123"))[..32].to_string();
        assert_eq!(connection.key, key);
        assert_eq!(connection.key.len(), 32);
        let folder = fixture.inner.state_home.join("connections").join(&key);
        assert_eq!(connection.dir, folder);
        assert_eq!(connection.repo(), folder.join("repo"));
        assert_ne!(
            fixture.connection("https://paper.example/docs/other").key,
            key
        );
        assert!(!connection.connected());
    }

    #[tokio::test]
    async fn connecting_clones_the_repository_and_records_the_project() {
        if !git_ready().await {
            return;
        }
        let fixture = Fixture::new().await;
        let bare = bare_remote(fixture.scratch.path());
        let connection = fixture.connection(PROJECT);

        let status = run_connect(&fixture.inner, &connection, text(&bare))
            .await
            .unwrap();
        assert_eq!(status["connected"], json!(true), "{status}");
        assert_eq!(status["branch"], json!("main"), "{status}");
        assert_eq!(status["merging"], Value::Null);
        assert_eq!(status["url"], json!(text(&bare)));
        assert!(connection.repo().join("main.tex").exists());
        assert!(!connection.dir.join(PARTIAL).exists());

        let record: Record =
            serde_json::from_slice(&std::fs::read(connection.dir.join(RECORD)).unwrap()).unwrap();
        assert_eq!(record.project, PROJECT);
        assert_eq!(record.url, text(&bare));

        let store = store_for(&connection.repo(), &fixture.inner.state_home);
        assert_eq!(store.load().unwrap().unwrap().link.slug, "abc123");
        assert_eq!(
            plain_git(
                &connection.repo(),
                &["config", "--file", ".git/config", "user.email"]
            ),
            MERGE_EMAIL
        );

        let (status, body) = run_connect(&fixture.inner, &connection, text(&bare))
            .await
            .unwrap_err();
        assert_eq!(
            (status, &body["reason"]),
            (409, &json!("connected")),
            "{body}"
        );

        // A token in an address is never shown back to the page.
        let record = Record {
            project: PROJECT.to_owned(),
            url: "https://ghp_secret@github.com/o/r.git".to_owned(),
        };
        std::fs::write(
            connection.dir.join(RECORD),
            serde_json::to_vec(&record).unwrap(),
        )
        .unwrap();
        let status = status_of(&fixture.inner, &connection).await.unwrap();
        assert_eq!(status["url"], json!("https://github.com/o/r.git"));
    }

    #[tokio::test]
    async fn an_empty_remote_has_nothing_to_pull_and_one_out_of_reach_says_so() {
        if !git_ready().await {
            return;
        }
        let fixture = Fixture::new().await;
        let scratch = fixture.scratch.path();
        plain_git(scratch, &["init", "--bare", "-b", "main", "empty.git"]);
        let empty = scratch.join("empty.git");
        let connection = fixture.connection(PROJECT);
        run_connect(&fixture.inner, &connection, text(&empty))
            .await
            .unwrap();

        let pulled = pull_remote(&connection.repo()).await.unwrap();
        assert_eq!(pulled["status"], json!("up-to-date"), "{pulled}");

        std::fs::remove_dir_all(&empty).unwrap();
        assert!(pull_remote(&connection.repo()).await.is_err());
    }

    #[tokio::test]
    async fn a_repository_linked_to_another_project_is_refused_and_leaves_no_folder() {
        if !git_ready().await {
            return;
        }
        let fixture = Fixture::new().await;
        let bare = bare_remote(fixture.scratch.path());
        // A clone that keeps the state of another project, pushed as
        // LibrePaper pushes it.
        let seed = fixture.scratch.path().join("seed");
        let other = Link::parse("https://paper.example/docs/other").unwrap();
        store_for(&seed, &fixture.inner.state_home)
            .save(&State::new(other))
            .unwrap();
        plain_git(
            &seed,
            &["push", "origin", "refs/librepaper/*:refs/librepaper/*"],
        );

        let connection = fixture.connection(PROJECT);
        let (status, body) = run_connect(&fixture.inner, &connection, text(&bare))
            .await
            .unwrap_err();
        assert_eq!((status, &body["reason"]), (409, &json!("linked")), "{body}");
        assert!(!connection.dir.exists());
    }

    #[tokio::test]
    async fn resolving_a_merge_keeps_the_chosen_side_and_commits_when_none_is_left() {
        if !git_ready().await {
            return;
        }
        let fixture = Fixture::new().await;
        let bare = bare_remote(fixture.scratch.path());
        let connection = fixture.connection(PROJECT);
        run_connect(&fixture.inner, &connection, text(&bare))
            .await
            .unwrap();
        let repo = connection.repo();

        let body = ResolveBody {
            project: PROJECT.to_string(),
            path: "main.tex".to_string(),
            keep: "repository".to_string(),
        };
        let (status, failure) = resolve(&fixture.inner, Some(PAGE), &body)
            .await
            .unwrap_err();
        assert_eq!((status, &failure["reason"]), (409, &json!("not-merging")));

        // Two branches edit the same line, and the merge is left in conflict.
        std::fs::write(repo.join("main.tex"), "ours\n").unwrap();
        plain_git(&repo, &["commit", "-am", "Ours"]);
        plain_git(&repo, &["checkout", "-b", "side", "HEAD~1"]);
        std::fs::write(repo.join("main.tex"), "theirs\n").unwrap();
        plain_git(&repo, &["commit", "-am", "Theirs"]);
        plain_git(&repo, &["checkout", "main"]);
        assert!(!plain_git_output(&repo, &["merge", "side"]).status.success());

        let status = status_of(&fixture.inner, &connection).await.unwrap();
        assert_eq!(status["merging"]["side"], json!("repository"), "{status}");
        assert_eq!(
            status["merging"]["conflicts"],
            json!(["main.tex"]),
            "{status}"
        );

        let status = resolve(&fixture.inner, Some(PAGE), &body).await.unwrap();
        assert_eq!(status["merging"], Value::Null, "{status}");
        assert_eq!(
            std::fs::read_to_string(repo.join("main.tex")).unwrap(),
            "theirs\n"
        );
        assert!(!repo.join(".git/MERGE_HEAD").exists());
        assert_eq!(
            plain_git(&repo, &["log", "-1", "--format=%s"]),
            "Merge branch 'side'"
        );
    }

    #[tokio::test]
    async fn a_merge_whose_commit_failed_is_committed_on_the_next_try() {
        if !git_ready().await {
            return;
        }
        let fixture = Fixture::new().await;
        let bare = bare_remote(fixture.scratch.path());
        let connection = fixture.connection(PROJECT);
        run_connect(&fixture.inner, &connection, text(&bare))
            .await
            .unwrap();
        let repo = connection.repo();
        std::fs::write(repo.join("main.tex"), "ours\n").unwrap();
        plain_git(&repo, &["commit", "-am", "Ours"]);
        plain_git(&repo, &["checkout", "-b", "side", "HEAD~1"]);
        std::fs::write(repo.join("main.tex"), "theirs\n").unwrap();
        plain_git(&repo, &["commit", "-am", "Theirs"]);
        plain_git(&repo, &["checkout", "main"]);
        assert!(!plain_git_output(&repo, &["merge", "side"]).status.success());

        // A hook refuses the commit that settles the last file.
        let hook = repo.join(".git/hooks/commit-msg");
        std::fs::write(&hook, "#!/bin/sh\necho refused >&2\nexit 1\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let body = ResolveBody {
            project: PROJECT.to_string(),
            path: "main.tex".to_string(),
            keep: "repository".to_string(),
        };
        assert!(resolve(&fixture.inner, Some(PAGE), &body).await.is_err());
        assert!(repo.join(".git/MERGE_HEAD").exists());
        assert!(
            unfinished_merge(&repo).await.is_err(),
            "the hook still refuses"
        );

        std::fs::remove_file(&hook).unwrap();
        assert!(!unfinished_merge(&repo).await.unwrap());
        assert!(!repo.join(".git/MERGE_HEAD").exists());
        let status = status_of(&fixture.inner, &connection).await.unwrap();
        assert_eq!(status["merging"], Value::Null, "{status}");
    }

    #[tokio::test]
    async fn disconnecting_removes_the_connection_folder() {
        let fixture = Fixture::new().await;
        let connection = fixture.connection(PROJECT);
        std::fs::create_dir_all(connection.repo()).unwrap();
        assert!(connection.connected());

        let status = disconnect(&fixture.inner, Some(PAGE), PROJECT)
            .await
            .unwrap();
        assert_eq!(status["connected"], json!(false), "{status}");
        assert!(!connection.dir.exists());
        let status = disconnect(&fixture.inner, Some(PAGE), PROJECT)
            .await
            .unwrap();
        assert_eq!(status["connected"], json!(false), "{status}");
    }

    #[tokio::test]
    async fn a_second_operation_on_a_connection_waits_for_the_first() {
        let fixture = Fixture::new().await;
        let connection = fixture.connection(PROJECT);
        std::fs::create_dir_all(connection.repo()).unwrap();
        let running = fixture.inner.connections.claim(&connection.key).unwrap();

        let (status, body) = disconnect(&fixture.inner, Some(PAGE), PROJECT)
            .await
            .unwrap_err();
        assert_eq!((status, &body["reason"]), (409, &json!("busy")), "{body}");
        assert!(connection.connected());

        drop(running);
        disconnect(&fixture.inner, Some(PAGE), PROJECT)
            .await
            .unwrap();
        assert!(!connection.connected());
    }

    #[tokio::test]
    async fn the_router_dispatches_the_connection_routes() {
        let fixture = Fixture::new().await;
        let (status, body) = fixture
            .send(Method::GET, &format!("connections?project={PROJECT}"), None)
            .await;
        assert_eq!((status, &body["connected"]), (200, &json!(false)), "{body}");

        let (status, body) = fixture
            .send(
                Method::POST,
                "connections/disconnect",
                Some(json!({"project": PROJECT})),
            )
            .await;
        assert_eq!((status, &body["connected"]), (200, &json!(false)), "{body}");

        let project = json!({"project": PROJECT});
        for (path, body) in [
            ("connections/pull", project.clone()),
            ("connections/push", project.clone()),
            (
                "connections/resolve",
                json!({"project": PROJECT, "path": "a.txt", "keep": "project"}),
            ),
        ] {
            let (status, reply) = fixture.send(Method::POST, path, Some(body)).await;
            assert_eq!(
                (status, &reply["reason"]),
                (409, &json!("not-connected")),
                "{path}: {reply}"
            );
        }

        for path in [
            "connections",
            "connections/login",
            "connections/login/complete",
        ] {
            let (status, _) = fixture.send(Method::POST, path, Some(json!({}))).await;
            assert_ne!(status, 404, "{path}");
        }
        let (status, _) = fixture.send(Method::POST, "connections/other", None).await;
        assert_eq!(status, 404);
    }
}
