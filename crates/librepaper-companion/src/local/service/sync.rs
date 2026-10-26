//! Sync routes on a folder project: keep a folder in step with a web project.
//!
//! The work is `librepaper_sync`'s, the same pull and push the `librepaper`
//! command line runs. This file authenticates, finds the folder, holds the
//! sign-in the device flow ends with, and shapes the answers. The link and the
//! hashes of what was last pulled or pushed live in the state home keyed by
//! the folder, never in the folder.
//!
//! A folder that is a Git repository syncs through Git, as the command line
//! does in a repository. Its link and hashes live in the repository, under
//! `refs/librepaper/state`, and a pull turns the web project's versions into
//! commits on `refs/librepaper/main` and has git merge them. A git folder's
//! pull never writes the working tree itself: git does, after the merge.
//!
//! A folder may only be linked to a project on the site of the page that asks,
//! so a page cannot spend the token this computer holds for another site. The
//! link lives with the folder, not with the page, so a page that opens a folder
//! linked from another site finds it unlinked and refused.
//! Pull and push touch no change-poller state: the files a pull writes are
//! outside changes like any other, and the poll hands them to the editor.
//!
//! A local render writes its result and whatever the tool leaves beside it
//! into the folder. The editor leaves those out of the project, and so does
//! every call into the sync crate here: the folder's recorded render outputs
//! are passed as the paths to exclude, so a push, a publish, a pull and the
//! count of changes never see them. The record is the one the change poller
//! keeps, in memory, per folder project.

use super::*;

use librepaper_base::http::Credentials;
use librepaper_sync::{
    changes_here, publish, pull, push, store_for, web_changed, Client, Error, Link, MergeOutcome,
    PullOptions, PullOutcome, PushOptions, PushOutcome, State, StateStore, VersionCommit,
};

use crate::local::bindings::ProjectBinding;
use crate::local::credentials;

/// How long a code waits to be approved, and how many wait at once.
const LOGIN_TTL: Duration = Duration::from_secs(10 * 60);
const MAX_PENDING_LOGINS: usize = 32;
/// The longest a call to the site's sign-in endpoints may take.
const LOGIN_TIMEOUT: Duration = Duration::from_secs(10);
const MAX_DEVICE_FIELD_BYTES: usize = 4096;

const NOT_LINKED: &str = "link this folder to a web project first";
const OTHER_SITE: &str = "this folder is linked to a project on another site";

/// A refusal: the status and the JSON body to answer with.
pub(super) type Failure = (u16, Value);

pub(super) fn refuse(status: u16, error: &str) -> Failure {
    (status, json!({"error": error}))
}

/// Whether the site refused the token this computer holds: it expired, was
/// revoked, or has no right to the project.
pub(super) fn token_refused(error: &Error) -> bool {
    matches!(
        error,
        Error::Http {
            status: 401 | 403,
            ..
        }
    )
}

/// A failed call into the sync crate. The crate's message already says what
/// went wrong in plain words. A refused token is the page's cue to sign in
/// again, so it answers 401 like a missing one; the site being unreachable or
/// answering badly is a 502; a mistake in what was asked is a 400; and the
/// folder, the state record or git failing here is a 500. A merge that is still
/// in progress is a 409 with the reason `merging`.
pub(super) fn failed(error: &Error) -> Failure {
    if token_refused(error) {
        return refuse(401, "sign in");
    }
    match error {
        Error::NotLinked => refuse(409, NOT_LINKED),
        Error::Merging => (
            409,
            json!({"reason": "merging", "error": error.to_string()}),
        ),
        Error::Link(_) | Error::Rules(_) | Error::Invalid(_) => refuse(400, &error.to_string()),
        Error::Network(_) | Error::Http { .. } | Error::Trimmed => refuse(502, &error.to_string()),
        Error::Io { .. } | Error::Git(_) | Error::State(_) => refuse(500, &error.to_string()),
    }
}

pub(super) fn answer(result: Result<Value, Failure>) -> Reply {
    match result {
        Ok(body) => write_json(200, &body),
        Err((status, body)) => write_json(status, &body),
    }
}

/// The folder behind a sync route, with the store that holds its link: the
/// repository's when the folder is a Git work tree, the state home's otherwise.
struct Folder {
    binding: ProjectBinding,
    store: Box<dyn StateStore>,
}

/// What the state home holds for a folder, as one page sees it.
enum Stored {
    Nothing,
    /// A link to a project on the page's own site.
    Here(Box<State>),
    /// A link to a project on another site. The token this computer holds for
    /// that site is not this page's to spend, so the page sees no link.
    OtherSite,
}

impl Folder {
    fn new(inner: &Inner, binding: ProjectBinding) -> Folder {
        let store = store_for(&binding.root, &inner.state_home);
        Folder { binding, store }
    }

    /// The folder's link as seen from `origin`. The state is keyed by the
    /// directory alone, while the binding that reached it is scoped by origin.
    fn stored(&self, origin: Option<&str>) -> Result<Stored, Failure> {
        let Some(state) = self.store.load().map_err(|error| failed(&error))? else {
            return Ok(Stored::Nothing);
        };
        Ok(if same_site(&state.link.site, &origin_site(origin)?) {
            Stored::Here(Box::new(state))
        } else {
            Stored::OtherSite
        })
    }

    fn state(&self, origin: Option<&str>) -> Result<Option<State>, Failure> {
        match self.stored(origin)? {
            Stored::Nothing => Ok(None),
            Stored::Here(state) => Ok(Some(*state)),
            Stored::OtherSite => Err(refuse(409, OTHER_SITE)),
        }
    }

    fn linked(&self, origin: Option<&str>) -> Result<State, Failure> {
        self.state(origin)?.ok_or_else(|| refuse(409, NOT_LINKED))
    }

    /// The files a render wrote into the folder, which the sync leaves alone.
    fn render_outputs(&self, inner: &Inner) -> Vec<String> {
        inner
            .projects
            .render_outputs(&self.binding.id)
            .into_iter()
            .collect()
    }
}

fn is_git_folder(root: &Path) -> bool {
    root.join(".git").exists()
}

/// The folder project `id` of the requesting origin, for a request that has
/// passed the credential check.
#[allow(clippy::result_large_err)]
fn open_folder(
    inner: &Inner,
    headers: &HeaderMap,
    origin: Option<&str>,
    id: &str,
) -> Result<Folder, Reply> {
    authenticate(inner, headers, origin)?;
    let binding = projects::open_binding(inner, origin, id)?;
    Ok(Folder::new(inner, binding))
}

fn site_text(site: &url::Url) -> &str {
    site.as_str().trim_end_matches('/')
}

/// The site the requesting page was served from.
pub(super) fn origin_site(origin: Option<&str>) -> Result<url::Url, Failure> {
    url::Url::parse(&store::normalize_origin(origin.unwrap_or_default()))
        .map_err(|_| refuse(400, "missing or invalid X-LibrePaper-Origin header"))
}

pub(super) fn same_site(left: &url::Url, right: &url::Url) -> bool {
    store::normalize_origin(left.as_str()) == store::normalize_origin(right.as_str())
}

/// A project link that names the requesting page's own site.
pub(super) fn parse_link(raw: &str, origin: Option<&str>) -> Result<Link, Failure> {
    let link = Link::parse(raw.trim()).map_err(|error| refuse(400, &error.to_string()))?;
    if !same_site(&link.site, &origin_site(origin)?) {
        return Err(refuse(400, "that project is on another site"));
    }
    Ok(link)
}

pub(super) fn project_url(link: &Link) -> String {
    format!("{}/docs/{}", site_text(&link.site), link.slug)
}

/// A client for `site` with the token this computer holds for it, or the 401
/// that asks the page to sign in.
pub(super) fn client_for(inner: &Inner, site: &url::Url) -> Result<Client, Failure> {
    let token = credentials::stored_token_at(&inner.state_home, site.as_str());
    if token.is_empty() {
        return Err(refuse(401, "sign in"));
    }
    Ok(Client::new(site.clone(), Credentials::token(&token)))
}

/* --------------------------------------------------------------- status */

/// Where the folder stands: whether it is linked and signed in, and when it
/// is both, how much changed here and whether the web project moved. A site
/// that cannot be reached leaves those two empty and says why in `error`, so
/// the folder can still be unlinked. A site that refuses the stored token
/// reads as signed out. A folder linked from another site reads as unlinked
/// and says so in `other_site`, without a call to either site.
async fn folder_status(
    inner: &Inner,
    folder: &Folder,
    origin: Option<&str>,
) -> Result<Value, Failure> {
    let (state, other_site) = match folder.stored(origin)? {
        Stored::Nothing => (None, false),
        Stored::Here(state) => (Some(Arc::new(*state)), false),
        Stored::OtherSite => (None, true),
    };
    let site = match &state {
        Some(state) => state.link.site.clone(),
        None => origin_site(origin)?,
    };
    let token = credentials::stored_token_at(&inner.state_home, site.as_str());
    let mut signed_in = !token.is_empty();
    let (mut here, mut web, mut problem) = (0, false, None);
    if let (Some(state), true) = (&state, signed_in) {
        let client = Client::new(site.clone(), Credentials::token(&token));
        let (rules, moved) = tokio::join!(client.rules(), web_changed(&client, state));
        let mut refused = false;
        match moved {
            Ok(moved) => web = moved,
            Err(error) if token_refused(&error) => refused = true,
            Err(error) => problem = Some(error.to_string()),
        }
        match rules {
            Ok(rules) => {
                let (root, state) = (folder.binding.root.clone(), state.clone());
                let exclude = folder.render_outputs(inner);
                match tokio::task::spawn_blocking(move || {
                    changes_here(&root, &state, &rules, &exclude)
                })
                .await
                {
                    Ok(Ok(count)) => here = count,
                    Ok(Err(error)) => problem = Some(error.to_string()),
                    Err(_) => problem = Some("the folder read stopped".to_string()),
                }
            }
            Err(error) => problem = Some(error.to_string()),
        }
        if refused {
            signed_in = false;
            problem = Some("sign in".to_string());
        }
    }
    let mut body = json!({
        "signed_in": signed_in,
        "linked": state.is_some(),
        "git": is_git_folder(&folder.binding.root),
        "site": state.as_ref().map(|state| site_text(&state.link.site)),
        "slug": state.as_ref().map(|state| state.link.slug.as_str()),
        "pulled": state.as_ref().and_then(|state| state.pulled),
        "changes": {"here": here, "web": web},
    });
    if other_site {
        body["other_site"] = Value::Bool(true);
    }
    if let Some(problem) = problem {
        body["error"] = Value::String(problem);
    }
    Ok(body)
}

pub(super) async fn handle_status(
    inner: &Arc<Inner>,
    headers: &HeaderMap,
    origin: Option<&str>,
    id: &str,
) -> Reply {
    let folder = match open_folder(inner, headers, origin, id) {
        Ok(folder) => folder,
        Err(response) => return response,
    };
    answer(folder_status(inner, &folder, origin).await)
}

/* --------------------------------------------------------------- sign in */

struct PendingLogin {
    origin: String,
    /// The folder's binding id, or `connection:<key>` for a connection.
    owner: String,
    site: url::Url,
    device_code: String,
    expires: Instant,
}

/// The sign-ins started and not yet finished. The page approves the code in
/// its own session, and the device code that claims the token never leaves
/// this process.
pub(super) struct Logins {
    pending: std::sync::Mutex<HashMap<String, PendingLogin>>,
}

impl Logins {
    pub(super) fn new() -> Logins {
        Logins {
            pending: std::sync::Mutex::new(HashMap::new()),
        }
    }

    fn live(&self) -> std::sync::MutexGuard<'_, HashMap<String, PendingLogin>> {
        let mut pending = self
            .pending
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        pending.retain(|_, login| login.expires > Instant::now());
        pending
    }
}

/// One JSON POST to a sign-in endpoint of `site`. The client never follows a
/// redirect, because the reply decides where a token is stored.
async fn device_post(site: &url::Url, path: &str, body: &Value) -> Result<(u16, Value), String> {
    let response = librepaper_base::http::provider_client()
        .post(format!("{}{path}", site_text(site)))
        .timeout(LOGIN_TIMEOUT)
        .json(body)
        .send()
        .await
        .map_err(|error| format!("could not reach the site: {error}"))?;
    let (status, bytes) = librepaper_base::http::read_capped(response).await?;
    Ok((
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    ))
}

fn device_field<'a>(reply: &'a Value, name: &str) -> Option<&'a str> {
    reply
        .get(name)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty() && value.len() <= MAX_DEVICE_FIELD_BYTES)
}

#[derive(serde::Deserialize, Default)]
struct LoginBody {
    #[serde(default)]
    site: Option<String>,
}

pub(super) async fn handle_login(
    inner: &Arc<Inner>,
    headers: &HeaderMap,
    origin: Option<&str>,
    id: &str,
    request: Request<Body>,
) -> Reply {
    let folder = match open_folder(inner, headers, origin, id) {
        Ok(folder) => folder,
        Err(response) => return response,
    };
    let body = match read_json_body::<LoginBody>(request).await {
        Ok(body) => body,
        Err(response) => return response,
    };
    let begun = async {
        let site = folder_site(&folder, origin, body.site)?;
        begin_login(inner, origin, site, &folder.binding.id).await
    };
    answer(begun.await)
}

/// The site a folder's sign-in is for: the site of its project when it is
/// linked, else the page's own, or `site` when that names it.
fn folder_site(
    folder: &Folder,
    origin: Option<&str>,
    site: Option<String>,
) -> Result<url::Url, Failure> {
    let page = origin_site(origin)?;
    match (folder.state(origin)?, site) {
        (Some(state), _) => Ok(state.link.site),
        (None, None) => Ok(page),
        (None, Some(raw)) => {
            let site =
                url::Url::parse(raw.trim()).map_err(|_| refuse(400, "that site is not a URL"))?;
            if !same_site(&site, &page) {
                return Err(refuse(400, "that is not the site of this page"));
            }
            Ok(site)
        }
    }
}

/// Asks `site` for a code on behalf of `owner`, a folder's binding id or
/// `connection:<key>`, and answers the code the page shows. A newer sign-in
/// for the same owner replaces the one before it.
pub(super) async fn begin_login(
    inner: &Inner,
    origin: Option<&str>,
    site: url::Url,
    owner: &str,
) -> Result<Value, Failure> {
    let (status, reply) = device_post(&site, "/api/auth/device", &json!({}))
        .await
        .map_err(|error| refuse(502, &format!("could not start the sign-in: {error}")))?;
    let (Some(device_code), Some(user_code)) = (
        device_field(&reply, "device_code"),
        device_field(&reply, "user_code"),
    ) else {
        return Err(refuse(
            502,
            &format!("could not start the sign-in: the site answered HTTP {status}"),
        ));
    };
    let authorization_id = librepaper_base::util::random_token();
    {
        let mut pending = inner.sync_logins.live();
        pending.retain(|_, login| login.owner != owner);
        if pending.len() >= MAX_PENDING_LOGINS {
            return Err(refuse(
                429,
                "too many sign-ins are pending; try again shortly",
            ));
        }
        pending.insert(
            authorization_id.clone(),
            PendingLogin {
                origin: store::normalize_origin(origin.unwrap_or_default()),
                owner: owner.to_owned(),
                site: site.clone(),
                device_code: device_code.to_owned(),
                expires: Instant::now() + LOGIN_TTL,
            },
        );
    }
    let mut body = json!({"authorization_id": authorization_id, "user_code": user_code});
    if let Some(url) = device_field(&reply, "verification_url") {
        body["verification_url"] = Value::String(url.to_owned());
    }
    Ok(body)
}

#[derive(serde::Deserialize)]
struct LoginCompleteBody {
    authorization_id: String,
}

pub(super) async fn handle_login_complete(
    inner: &Arc<Inner>,
    headers: &HeaderMap,
    origin: Option<&str>,
    id: &str,
    request: Request<Body>,
) -> Reply {
    let folder = match open_folder(inner, headers, origin, id) {
        Ok(folder) => folder,
        Err(response) => return response,
    };
    let body = match read_json_body::<LoginCompleteBody>(request).await {
        Ok(body) => body,
        Err(response) => return response,
    };
    let finished = async {
        finish_login(inner, origin, &folder.binding.id, &body.authorization_id).await?;
        folder_status(inner, &folder, origin).await
    };
    answer(finished.await)
}

/// Claims the token once the page has approved the code for `owner`, stores it
/// under the site's origin as the backups and the command line do, and answers
/// the site it was stored for. A code not yet approved is a 400 the page
/// retries.
pub(super) async fn finish_login(
    inner: &Inner,
    origin: Option<&str>,
    owner: &str,
    authorization_id: &str,
) -> Result<url::Url, Failure> {
    let (site, device_code) = {
        let pending = inner.sync_logins.live();
        let Some(login) = pending.get(authorization_id) else {
            return Err(refuse(
                400,
                "the sign-in expired or was replaced; start again",
            ));
        };
        if login.origin != store::normalize_origin(origin.unwrap_or_default())
            || login.owner != owner
        {
            return Err(refuse(400, "the sign-in does not belong to this folder"));
        }
        (login.site.clone(), login.device_code.clone())
    };
    let (code, reply) = device_post(
        &site,
        "/api/auth/device/token",
        &json!({"device_code": device_code}),
    )
    .await
    .map_err(|error| refuse(502, &format!("could not finish the sign-in: {error}")))?;
    let token = match device_field(&reply, "token") {
        Some(token) if code == 200 => token.to_owned(),
        _ => {
            let reason = reply.get("error").and_then(Value::as_str);
            if reason == Some("authorization_pending") {
                return Err(refuse(400, "sign in and approve the code, then retry"));
            }
            inner.sync_logins.live().remove(authorization_id);
            return Err(match reason {
                Some("expired_token") => refuse(400, "the sign-in expired; start again"),
                _ => refuse(
                    502,
                    &format!("could not finish the sign-in: the site answered HTTP {code}"),
                ),
            });
        }
    };
    let base = inner.state_home.clone();
    let stored = site.clone();
    tokio::task::spawn_blocking(move || {
        credentials::store_token_at(&base, stored.as_str(), &token)
    })
    .await
    .map_err(|error| refuse(500, &format!("could not save the sign-in: {error}")))?
    .map_err(|error| refuse(500, &error))?;
    inner.sync_logins.live().remove(authorization_id);
    Ok(site)
}

/* ---------------------------------------------------------- link, unlink */

#[derive(serde::Deserialize)]
struct LinkBody {
    url: String,
}

pub(super) async fn handle_link(
    inner: &Arc<Inner>,
    headers: &HeaderMap,
    origin: Option<&str>,
    id: &str,
    request: Request<Body>,
) -> Reply {
    let folder = match open_folder(inner, headers, origin, id) {
        Ok(folder) => folder,
        Err(response) => return response,
    };
    let body = match read_json_body::<LinkBody>(request).await {
        Ok(body) => body,
        Err(response) => return response,
    };
    let linked = async {
        let link = parse_link(&body.url, origin)?;
        record_link(&folder, link)?;
        folder_status(inner, &folder, origin).await
    };
    answer(linked.await)
}

/// Starts the folder's sync state at `link`: nothing pulled, no file recorded.
fn record_link(folder: &Folder, link: Link) -> Result<(), Failure> {
    folder
        .store
        .save(&State::new(link))
        .map_err(|error| failed(&error))
}

pub(super) async fn handle_unlink(
    inner: &Arc<Inner>,
    headers: &HeaderMap,
    origin: Option<&str>,
    id: &str,
) -> Reply {
    let folder = match open_folder(inner, headers, origin, id) {
        Ok(folder) => folder,
        Err(response) => return response,
    };
    let unlinked = async {
        folder.store.clear().map_err(|error| failed(&error))?;
        folder_status(inner, &folder, origin).await
    };
    answer(unlinked.await)
}

/* -------------------------------------------------------------- publish */

#[derive(serde::Deserialize)]
struct PublishBody {
    title: String,
}

pub(super) async fn handle_publish(
    inner: &Arc<Inner>,
    headers: &HeaderMap,
    origin: Option<&str>,
    id: &str,
    request: Request<Body>,
) -> Reply {
    let folder = match open_folder(inner, headers, origin, id) {
        Ok(folder) => folder,
        Err(response) => return response,
    };
    let body = match read_json_body::<PublishBody>(request).await {
        Ok(body) => body,
        Err(response) => return response,
    };
    answer(publish_folder(inner, &folder, origin, body.title.trim()).await)
}

/// Makes the folder a new web project on the page's own site.
async fn publish_folder(
    inner: &Inner,
    folder: &Folder,
    origin: Option<&str>,
    title: &str,
) -> Result<Value, Failure> {
    if title.is_empty() {
        return Err(refuse(400, "a project needs a title"));
    }
    if folder.state(origin)?.is_some() {
        return Err(refuse(
            409,
            "this folder is already linked to a web project",
        ));
    }
    let client = Arc::new(client_for(inner, &origin_site(origin)?)?);
    let exclude = folder.render_outputs(inner);
    let (root, state_home) = (folder.binding.root.clone(), inner.state_home.clone());
    let title = title.to_owned();
    let handle = tokio::runtime::Handle::current();
    let link = tokio::task::spawn_blocking(move || {
        let store = store_for(&root, &state_home);
        handle.block_on(publish(&client, &root, store.as_ref(), &title, &exclude))
    })
    .await
    .map_err(|_| refuse(500, "the publish stopped"))?
    .map_err(|error| failed(&error))?;
    let mut body = folder_status(inner, folder, origin).await?;
    body["url"] = Value::String(project_url(&link));
    Ok(body)
}

/* ----------------------------------------------------------- pull, push */

#[derive(serde::Deserialize, Default)]
struct PullBody {
    #[serde(default)]
    mine: Vec<String>,
    #[serde(default)]
    theirs: Vec<String>,
}

pub(super) async fn handle_pull(
    inner: &Arc<Inner>,
    headers: &HeaderMap,
    origin: Option<&str>,
    id: &str,
    request: Request<Body>,
) -> Reply {
    let folder = match open_folder(inner, headers, origin, id) {
        Ok(folder) => folder,
        Err(response) => return response,
    };
    let body = match read_json_body::<PullBody>(request).await {
        Ok(body) => body,
        Err(response) => return response,
    };
    let PullBody { mine, theirs } = body;
    if !mine
        .iter()
        .chain(&theirs)
        .all(|path| protocol::safe_relative_path(path))
    {
        return write_json(400, &json!({"error": "unsafe file path"}));
    }
    answer(run_pull(inner, &folder, origin, mine, theirs).await)
}

/// Takes the web project's newest version into the folder. A file changed on
/// both sides is not written; `mine` and `theirs` settle those, per path. In a
/// git folder the versions become commits and git merges them, so `mine` and
/// `theirs` are not read there.
async fn run_pull(
    inner: &Inner,
    folder: &Folder,
    origin: Option<&str>,
    mine: Vec<String>,
    theirs: Vec<String>,
) -> Result<Value, Failure> {
    let state = folder.linked(origin)?;
    let client = Arc::new(client_for(inner, &state.link.site)?);
    let options = PullOptions {
        merge: true,
        mine,
        theirs,
        exclude: folder.render_outputs(inner),
    };
    let (root, state_home) = (folder.binding.root.clone(), inner.state_home.clone());
    let handle = tokio::runtime::Handle::current();
    let outcome = tokio::task::spawn_blocking(move || {
        let store = store_for(&root, &state_home);
        handle.block_on(pull(&client, &root, store.as_ref(), &options))
    })
    .await
    .map_err(|_| refuse(500, "the pull stopped"))?
    .map_err(|error| failed(&error))?;
    match outcome {
        PullOutcome::Folder {
            written,
            trashed,
            kept,
            conflicts,
            skipped,
            trimmed,
        } => Ok(json!({
            "written": written,
            "trashed": trashed,
            "kept": kept,
            "conflicts": conflicts,
            "skipped": skipped,
            "trimmed": trimmed,
        })),
        PullOutcome::Repository {
            versions,
            merge,
            trimmed,
        } => Ok(repository_reply(&versions, &merge, trimmed)),
        PullOutcome::UpToDate if folder.store.repository().is_some() => {
            Ok(repository_reply(&[], &MergeOutcome::AlreadyMerged, false))
        }
        PullOutcome::UpToDate => Ok(json!({
            "written": [],
            "trashed": [],
            "kept": [],
            "conflicts": [],
            "skipped": [],
            "trimmed": false,
        })),
    }
}

/// A pull in a git folder as the page reads it: the web versions that became
/// commits on `refs/librepaper/main`, and what git did with them.
fn repository_reply(versions: &[VersionCommit], merge: &MergeOutcome, trimmed: bool) -> Value {
    let merge = match merge {
        MergeOutcome::Skipped => json!("skipped"),
        MergeOutcome::FastForward => json!("fast-forward"),
        MergeOutcome::Merged => json!("merged"),
        MergeOutcome::AlreadyMerged => json!("already"),
        MergeOutcome::Conflicts(paths) => json!({"conflicts": paths}),
        MergeOutcome::DirtyTree(paths) => json!({"dirty": paths}),
    };
    let versions: Vec<Value> = versions
        .iter()
        .map(|version| {
            json!({
                "sha": version.sha,
                "label": version.label,
                "by": version.by,
                "commit": version.commit,
            })
        })
        .collect();
    json!({
        "repository": true,
        "versions": versions,
        "merge": merge,
        "trimmed": trimmed,
    })
}

#[derive(serde::Deserialize)]
struct PushBody {
    message: String,
}

pub(super) async fn handle_push(
    inner: &Arc<Inner>,
    headers: &HeaderMap,
    origin: Option<&str>,
    id: &str,
    request: Request<Body>,
) -> Reply {
    let folder = match open_folder(inner, headers, origin, id) {
        Ok(folder) => folder,
        Err(response) => return response,
    };
    let body = match read_json_body::<PushBody>(request).await {
        Ok(body) => body,
        Err(response) => return response,
    };
    answer(run_push(inner, &folder, origin, body.message.trim()).await)
}

/// Sends what changed in the folder as one version. The page has saved its
/// edits to the folder first, so the files are what the editor holds.
async fn run_push(
    inner: &Inner,
    folder: &Folder,
    origin: Option<&str>,
    message: &str,
) -> Result<Value, Failure> {
    if message.is_empty() {
        return Err(refuse(400, "a push needs a message"));
    }
    let state = folder.linked(origin)?;
    let client = Arc::new(client_for(inner, &state.link.site)?);
    let options = PushOptions {
        message: message.to_owned(),
        author: None,
        exclude: folder.render_outputs(inner),
    };
    let (root, state_home) = (folder.binding.root.clone(), inner.state_home.clone());
    let handle = tokio::runtime::Handle::current();
    let outcome = tokio::task::spawn_blocking(move || {
        let store = store_for(&root, &state_home);
        handle.block_on(push(&client, &root, store.as_ref(), &options))
    })
    .await
    .map_err(|_| refuse(500, "the push stopped"))?
    .map_err(|error| failed(&error))?;
    match outcome {
        PushOutcome::Nothing => Ok(json!({"nothing": true})),
        PushOutcome::Pushed {
            label_sha,
            sequence,
            files,
        } => Ok(json!({
            "label": {"sha": label_sha, "sequence": sequence},
            "files": files,
        })),
        PushOutcome::Refused { paths } => Err((409, json!({"reason": "changed", "paths": paths}))),
        PushOutcome::Suggestion { path } => {
            Err((409, json!({"reason": "suggestion", "path": path})))
        }
    }
}

/* ----------------------------------------------------------------- keep */

#[derive(serde::Deserialize)]
struct KeepBody {
    url: String,
}

fn is_empty_dir(root: &Path) -> bool {
    std::fs::read_dir(root).is_ok_and(|mut entries| entries.next().is_none())
}

/// Whether a folder the person chose can take a project: an empty folder, or a
/// Git repository whose state the link is read from.
fn is_keepable_folder(root: &Path) -> bool {
    is_empty_dir(root) || is_git_folder(root)
}

/// Links the folder of `binding` to `link` and pulls it. A folder with no link
/// takes `link`. A folder already linked to the same project is pulled as it
/// stands, with its recorded state, and is not linked again. A folder linked to
/// another project is refused with the reason `linked` and not pulled. Once the
/// folder is opened, a failure still answers with the folder's `id`, so the page
/// can open it and try again from its Sync section, or sign in first.
pub(super) async fn link_and_pull(
    inner: &Inner,
    binding: ProjectBinding,
    origin: Option<&str>,
    link: Link,
) -> Reply {
    let mut opened = projects::opened(&binding);
    let folder = Folder::new(inner, binding);
    let pulled = async {
        match folder.stored(origin)? {
            Stored::Nothing => record_link(&folder, link)?,
            Stored::Here(state) if state.link.slug == link.slug => {}
            Stored::Here(_) | Stored::OtherSite => {
                return Err((
                    409,
                    json!({
                        "reason": "linked",
                        "error": "this repository is linked to another LibrePaper project",
                    }),
                ));
            }
        }
        run_pull(inner, &folder, origin, Vec::new(), Vec::new()).await
    };
    match pulled.await {
        Ok(pulled) => {
            opened["pull"] = pulled;
            write_json(200, &opened)
        }
        Err((status, mut failure)) => {
            if let (Some(into), Some(from)) = (failure.as_object_mut(), opened.as_object()) {
                into.extend(from.clone());
            }
            write_json(status, &failure)
        }
    }
}

/// Keeps a web project in a folder the person chooses, an empty one or a clone
/// of the repository, opens it as a folder project, links it and pulls. The
/// link and the pull are `link_and_pull`'s.
pub(super) async fn handle_keep(
    inner: &Arc<Inner>,
    headers: &HeaderMap,
    origin: Option<&str>,
    request: Request<Body>,
) -> Reply {
    if let Err(response) = authenticate(inner, headers, origin) {
        return response;
    }
    let body = match read_json_body::<KeepBody>(request).await {
        Ok(body) => body,
        Err(response) => return response,
    };
    let link = match parse_link(&body.url, origin) {
        Ok(link) => link,
        Err((status, body)) => return write_json(status, &body),
    };
    let root =
        match projects::choose_root(inner, "Choose an empty folder or a clone of the repository")
            .await
        {
            Ok(root) => root,
            Err(response) => return response,
        };
    if !is_keepable_folder(&root) {
        return write_json(
            409,
            &json!({"error": "choose an empty folder or a git repository"}),
        );
    }
    let binding = match inner
        .bindings
        .open_project(origin.unwrap_or_default(), &root)
    {
        Ok(binding) => binding,
        Err(error) => return write_json(400, &json!({"error": error})),
    };
    link_and_pull(inner, binding, origin, link).await
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    use axum::routing::{get, post};
    use axum::Json;
    use librepaper_sync::FileStore;

    const PAGE: &str = "https://paper.example";

    /// A folder project opened for `origin` in a fresh state home.
    struct Fixture {
        inner: Arc<Inner>,
        headers: HeaderMap,
        origin: String,
        binding: ProjectBinding,
        _state: Arc<tempfile::TempDir>,
        _root: tempfile::TempDir,
    }

    impl Fixture {
        async fn new(origin: &str) -> Fixture {
            let state = Arc::new(tempfile::tempdir().unwrap());
            let inner = test_support::inner(state.path(), state.path()).await;
            Fixture::open(inner, state, origin)
        }

        fn open(inner: Arc<Inner>, state: Arc<tempfile::TempDir>, origin: &str) -> Fixture {
            let root = tempfile::tempdir().unwrap();
            let binding = inner.bindings.open_project(origin, root.path()).unwrap();
            Fixture {
                headers: test_support::bearer(&inner),
                inner,
                origin: origin.to_string(),
                binding,
                _state: state,
                _root: root,
            }
        }

        /// A second folder project of the same origin in the same state home.
        fn another_folder(&self) -> Fixture {
            Fixture::open(self.inner.clone(), self._state.clone(), &self.origin)
        }

        fn root(&self) -> &Path {
            &self.binding.root
        }

        fn sign_in(&self, token: &str) {
            credentials::store_token_at(&self.inner.state_home, &self.origin, token).unwrap();
        }

        /// Records a link without asking the site, as `link` does.
        fn link_to(&self, url: &str) {
            record_link(
                &Folder::new(&self.inner, self.binding.clone()),
                Link::parse(url).unwrap(),
            )
            .unwrap();
        }

        /// Records the commit the folder is based on, as publishing does, so
        /// the versions a pull fetches are built on it.
        fn based_on(&self, head: &str) {
            let folder = Folder::new(&self.inner, self.binding.clone());
            let mut state = folder.store.load().unwrap().unwrap();
            state.head = Some(head.to_string());
            folder.store.save(&state).unwrap();
        }

        async fn status(&self) -> (u16, Value) {
            let reply = handle_status(&self.inner, &self.headers, self.origin(), self.id());
            read(reply.await).await
        }

        async fn link(&self, url: &str) -> (u16, Value) {
            let request = json_request(json!({"url": url}));
            let reply = handle_link(
                &self.inner,
                &self.headers,
                self.origin(),
                self.id(),
                request,
            );
            read(reply.await).await
        }

        async fn unlink(&self) -> (u16, Value) {
            let reply = handle_unlink(&self.inner, &self.headers, self.origin(), self.id());
            read(reply.await).await
        }

        async fn publish(&self, body: Value) -> (u16, Value) {
            let request = json_request(body);
            let reply = handle_publish(
                &self.inner,
                &self.headers,
                self.origin(),
                self.id(),
                request,
            );
            read(reply.await).await
        }

        async fn pull(&self, body: Value) -> (u16, Value) {
            let request = json_request(body);
            let reply = handle_pull(
                &self.inner,
                &self.headers,
                self.origin(),
                self.id(),
                request,
            );
            read(reply.await).await
        }

        async fn push(&self, body: Value) -> (u16, Value) {
            let request = json_request(body);
            let reply = handle_push(
                &self.inner,
                &self.headers,
                self.origin(),
                self.id(),
                request,
            );
            read(reply.await).await
        }

        async fn login(&self, body: Value) -> (u16, Value) {
            let request = json_request(body);
            let reply = handle_login(
                &self.inner,
                &self.headers,
                self.origin(),
                self.id(),
                request,
            );
            read(reply.await).await
        }

        async fn complete_login(&self, authorization_id: &str) -> (u16, Value) {
            let request = json_request(json!({"authorization_id": authorization_id}));
            let reply = handle_login_complete(
                &self.inner,
                &self.headers,
                self.origin(),
                self.id(),
                request,
            );
            read(reply.await).await
        }

        /// One request through the router, as the desktop shell sends it.
        async fn route(&self, method: Method, path: &str, body: Option<Value>) -> (u16, Value) {
            let builder = Request::builder()
                .method(method)
                .uri(format!("{BASE_PATH}/projects/{}/{path}", self.binding.id))
                .header("host", format!("127.0.0.1:{}", test_support::PORT))
                .header(
                    "authorization",
                    format!("Bearer {}", test_support::credential(&self.inner)),
                )
                .header("x-librepaper-origin", &self.origin);
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
            );
            read(reply.await).await
        }

        fn id(&self) -> &str {
            &self.binding.id
        }

        fn origin(&self) -> Option<&str> {
            Some(&self.origin)
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

    fn json_request(body: Value) -> Request<Body> {
        Request::post("/sync")
            .header("content-type", "application/json")
            .body(Body::from(body.to_string()))
            .unwrap()
    }

    fn assert_error(answer: &(u16, Value), status: u16, needle: &str) {
        assert_eq!(answer.0, status, "{}", answer.1);
        let error = answer.1["error"].as_str().unwrap_or_default();
        assert!(error.contains(needle), "{error}");
    }

    #[tokio::test]
    async fn an_unlinked_folder_reports_nothing_to_sync() {
        let fixture = Fixture::new(PAGE).await;
        let (status, body) = fixture.status().await;
        assert_eq!(status, 200);
        assert_eq!(
            body,
            json!({
                "signed_in": false,
                "linked": false,
                "git": false,
                "site": null,
                "slug": null,
                "pulled": null,
                "changes": {"here": 0, "web": false},
            })
        );

        // A token for the page's site is a sign-in before there is a link.
        fixture.sign_in("lp_token");
        let (_, body) = fixture.status().await;
        assert_eq!(body["signed_in"], true);
        assert_eq!(body["linked"], false);
    }

    #[tokio::test]
    async fn linking_records_the_project_and_unlinking_clears_it() {
        let fixture = Fixture::new(PAGE).await;
        let (status, body) = fixture.link("https://paper.example/docs/abc123").await;
        assert_eq!(status, 200, "{body}");
        assert_eq!(body["linked"], true);
        assert_eq!(body["site"], PAGE);
        assert_eq!(body["slug"], "abc123");
        assert_eq!(body["pulled"], Value::Null);
        // Not signed in, so nothing was asked of the site.
        assert_eq!(body["signed_in"], false);
        assert_eq!(body["changes"], json!({"here": 0, "web": false}));

        let store = FileStore::for_dir(&fixture.inner.state_home, fixture.root());
        let state = store.load().unwrap().unwrap();
        assert_eq!(state.link.slug, "abc123");
        assert!(state.files.is_empty());
        // The folder itself is untouched.
        assert_eq!(std::fs::read_dir(fixture.root()).unwrap().count(), 0);

        let (status, body) = fixture.status().await;
        assert_eq!((status, &body["linked"]), (200, &json!(true)));

        let (status, body) = fixture.unlink().await;
        assert_eq!(status, 200, "{body}");
        assert_eq!(body["linked"], false);
        assert_eq!(body["site"], Value::Null);
        assert!(store.load().unwrap().is_none());
    }

    #[tokio::test]
    async fn a_link_must_be_a_project_on_the_pages_own_site() {
        let fixture = Fixture::new(PAGE).await;
        let other = fixture.link("https://other.example/docs/abc123").await;
        assert_error(&other, 400, "another site");
        let garbage = fixture.link("not a project").await;
        assert_eq!(garbage.0, 400, "{}", garbage.1);
        let (_, body) = fixture.status().await;
        assert_eq!(body["linked"], false);
    }

    #[tokio::test]
    async fn another_origin_cannot_reach_the_folder() {
        let fixture = Fixture::new(PAGE).await;
        let reply = handle_status(
            &fixture.inner,
            &fixture.headers,
            Some("https://other.example"),
            fixture.id(),
        )
        .await;
        assert_eq!(read(reply).await.0, 404);
    }

    #[tokio::test]
    async fn pull_and_push_need_a_link_and_a_sign_in() {
        let fixture = Fixture::new(PAGE).await;
        let pull = fixture.pull(json!({})).await;
        assert_error(&pull, 409, "link this folder");
        let push = fixture.push(json!({"message": "Edits"})).await;
        assert_error(&push, 409, "link this folder");

        fixture.link_to("https://paper.example/docs/abc123");
        let pull = fixture.pull(json!({})).await;
        assert_eq!((pull.0, &pull.1), (401, &json!({"error": "sign in"})));
        let push = fixture.push(json!({"message": "Edits"})).await;
        assert_eq!((push.0, &push.1), (401, &json!({"error": "sign in"})));

        let silent = fixture.push(json!({"message": "  "})).await;
        assert_error(&silent, 400, "message");
        let unsafe_path = fixture.pull(json!({"mine": ["../outside.tex"]})).await;
        assert_error(&unsafe_path, 400, "unsafe");
    }

    #[tokio::test]
    async fn publishing_needs_a_title_a_sign_in_and_no_link() {
        let fixture = Fixture::new(PAGE).await;
        let untitled = fixture.publish(json!({"title": " "})).await;
        assert_error(&untitled, 400, "title");
        let signed_out = fixture.publish(json!({"title": "Paper"})).await;
        assert_eq!(signed_out.0, 401, "{}", signed_out.1);

        fixture.link_to("https://paper.example/docs/abc123");
        let linked = fixture.publish(json!({"title": "Paper"})).await;
        assert_error(&linked, 409, "already linked");
    }

    /// Runs git in `root` and answers what it printed, or `None` when git is
    /// missing or the command fails.
    fn git(root: &Path, args: &[&str]) -> Option<String> {
        let output = std::process::Command::new("git")
            .arg("-C")
            .arg(root)
            .args(args)
            .output()
            .ok()?;
        output
            .status
            .success()
            .then(|| String::from_utf8_lossy(&output.stdout).trim().to_string())
    }

    /// Makes the fixture's folder a git repository on `main` with one commit
    /// that holds `main.tex`, and answers that commit. `None` when git is not
    /// installed, so the test that needs one skips.
    fn init_repository(fixture: &Fixture) -> Option<String> {
        let root = fixture.root();
        git(root, &["init", "-q", "-b", "main"])?;
        std::fs::write(root.join("main.tex"), "main\n").unwrap();
        git(root, &["add", "-A"])?;
        let committed = [
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.org",
            "-c",
            "commit.gpgsign=false",
            "commit",
            "-q",
            "-m",
            "First",
        ];
        git(root, &committed)?;
        git(root, &["rev-parse", "HEAD"])
    }

    fn main_tex(root: &Path) -> String {
        std::fs::read_to_string(root.join("main.tex")).unwrap()
    }

    /// A site that serves the path rules and one named version, `v1` by
    /// Alice, whose `main.tex` reads `text`.
    async fn one_version_site(text: &str) -> (String, tokio::task::JoinHandle<()>) {
        let version = json!({
            "files": {"main.tex": {"kind": "text"}},
            "texts": {"main.tex": text},
        });
        let app = axum::Router::new()
            .route("/api/config", get(rules_reply))
            .fallback(move |uri: axum::http::Uri| {
                let version = version.clone();
                async move {
                    match uri.path() {
                        "/api/documents/abc123/history" => (
                            StatusCode::OK,
                            Json(json!({
                                "labels": [{
                                    "sha": "v1",
                                    "sequence": 1,
                                    "label": "First edit",
                                    "by": "Alice",
                                }],
                                "next_cursor": null,
                            })),
                        ),
                        "/api/documents/abc123/history/v1" => (StatusCode::OK, Json(version)),
                        _ => (
                            StatusCode::NOT_FOUND,
                            Json(json!({"error": "no such file"})),
                        ),
                    }
                }
            });
        serve(app).await
    }

    #[tokio::test]
    async fn a_git_folder_keeps_its_link_in_the_repository() {
        let fixture = Fixture::new(PAGE).await;
        if init_repository(&fixture).is_none() {
            return;
        }

        let (status, body) = fixture.link("https://paper.example/docs/abc123").await;
        assert_eq!(status, 200, "{body}");
        assert_eq!(body["linked"], true);
        assert_eq!(body["git"], true);
        let (_, body) = fixture.status().await;
        assert_eq!(body["linked"], true);
        assert_eq!(body["git"], true);

        // The state is a ref in the repository, and the state home holds none.
        let root = fixture.root();
        assert!(git(root, &["rev-parse", "--verify", "refs/librepaper/state"]).is_some());
        let files = FileStore::for_dir(&fixture.inner.state_home, root);
        assert!(files.load().unwrap().is_none());

        let (status, body) = fixture.unlink().await;
        assert_eq!(status, 200, "{body}");
        assert_eq!(body["linked"], false);
        assert_eq!(body["git"], true);
        assert!(git(root, &["rev-parse", "--verify", "refs/librepaper/state"]).is_none());
    }

    #[tokio::test]
    async fn a_git_folder_pulls_a_version_as_a_commit_and_fast_forwards() {
        let (site, _task) = one_version_site("main\nedited on the web\n").await;
        let fixture = Fixture::new(&site).await;
        let Some(head) = init_repository(&fixture) else {
            return;
        };
        fixture.sign_in("lp_token");
        fixture.link_to(&format!("{site}/docs/abc123"));
        fixture.based_on(&head);

        let (status, body) = fixture.pull(json!({})).await;
        assert_eq!(status, 200, "{body}");
        let root = fixture.root();
        let chain = git(root, &["rev-parse", "refs/librepaper/main"]).unwrap();
        assert_ne!(chain, head);
        assert_eq!(
            body,
            json!({
                "repository": true,
                "versions": [{
                    "sha": "v1",
                    "label": "First edit",
                    "by": "Alice",
                    "commit": chain,
                }],
                "merge": "fast-forward",
                "trimmed": false,
            })
        );

        // Git wrote the working tree: the branch moved to the version's commit.
        assert_eq!(git(root, &["rev-parse", "main"]).unwrap(), chain);
        assert_eq!(main_tex(root), "main\nedited on the web\n");
        let (_, body) = fixture.status().await;
        assert_eq!(body["pulled"], 1);

        // Nothing new on the web: the repository shape answers, with no versions.
        let (status, body) = fixture.pull(json!({})).await;
        assert_eq!(status, 200, "{body}");
        assert_eq!(
            body,
            json!({
                "repository": true,
                "versions": [],
                "merge": "already",
                "trimmed": false,
            })
        );
    }

    #[tokio::test]
    async fn a_linked_folder_that_becomes_a_repository_merges_on_its_first_pull() {
        let (site, _task) = one_version_site("main\nedited on the web\n").await;
        let fixture = Fixture::new(&site).await;
        fixture.sign_in("lp_token");
        fixture.link_to(&format!("{site}/docs/abc123"));
        if init_repository(&fixture).is_none() {
            return;
        }

        // The repository and the project share no commit, so the first merge
        // joins them, and main.tex differs on both sides.
        let (status, body) = fixture.pull(json!({})).await;
        assert_eq!(status, 200, "{body}");
        assert_eq!(body["repository"], true);
        assert_eq!(body["merge"], json!({"conflicts": ["main.tex"]}));
        let root = fixture.root();
        assert!(git(root, &["rev-parse", "-q", "--verify", "MERGE_HEAD"]).is_some());

        // The conflict markers are in main.tex, so the push is refused before
        // it asks the site for anything. The fake site does not count requests.
        let (status, body) = fixture.push(json!({"message": "Edits"})).await;
        assert_eq!(
            (status, &body["reason"]),
            (409, &json!("merging")),
            "{body}"
        );
    }

    #[tokio::test]
    async fn a_git_folder_with_uncommitted_changes_is_not_merged_into() {
        let (site, _task) = one_version_site("main\nedited on the web\n").await;
        let fixture = Fixture::new(&site).await;
        let Some(head) = init_repository(&fixture) else {
            return;
        };
        fixture.sign_in("lp_token");
        fixture.link_to(&format!("{site}/docs/abc123"));
        fixture.based_on(&head);
        let root = fixture.root();
        std::fs::write(root.join("main.tex"), "main\nedited here\n").unwrap();

        let (status, body) = fixture.pull(json!({})).await;
        assert_eq!(status, 200, "{body}");
        assert_eq!(
            body,
            json!({
                "repository": true,
                "versions": [],
                "merge": {"dirty": ["main.tex"]},
                "trimmed": false,
            })
        );

        // Nothing was fetched, and the file is as the person left it.
        assert!(git(root, &["rev-parse", "--verify", "refs/librepaper/main"]).is_none());
        assert_eq!(git(root, &["rev-parse", "main"]).unwrap(), head);
        assert_eq!(main_tex(root), "main\nedited here\n");
    }

    #[tokio::test]
    async fn a_git_folder_can_be_published() {
        let uploaded = Arc::new(std::sync::Mutex::new(String::new()));
        let held = uploaded.clone();
        let app = axum::Router::new()
            .route("/api/config", get(rules_reply))
            .route(
                "/api/documents",
                post(move |body: axum::body::Bytes| {
                    let held = held.clone();
                    async move {
                        *held.lock().unwrap() = String::from_utf8_lossy(&body).into_owned();
                        Json(json!({"slug": "abc123"}))
                    }
                }),
            );
        let (site, _task) = serve(app).await;
        let fixture = Fixture::new(&site).await;
        let Some(head) = init_repository(&fixture) else {
            return;
        };
        fixture.sign_in("lp_token");

        let (status, body) = fixture.publish(json!({"title": "Paper"})).await;
        assert_eq!(status, 200, "{body}");
        assert_eq!(body["url"], format!("{site}/docs/abc123"));
        assert_eq!(body["linked"], true);
        assert_eq!(body["git"], true);
        let sent = uploaded.lock().unwrap().clone();
        assert!(sent.contains("filename=\"main.tex\""), "{sent}");

        // The link is in the repository, and records the commit it was made at.
        let stored = store_for(fixture.root(), &fixture.inner.state_home);
        let state = stored.load().unwrap().unwrap();
        assert_eq!(state.link.slug, "abc123");
        assert_eq!(state.head.as_deref(), Some(head.as_str()));
    }

    #[tokio::test]
    async fn the_router_dispatches_the_sync_routes() {
        let fixture = Fixture::new(PAGE).await;
        let (status, body) = fixture.route(Method::GET, "sync", None).await;
        assert_eq!((status, &body["linked"]), (200, &json!(false)));

        let url = json!({"url": "https://paper.example/docs/abc123"});
        let (status, body) = fixture.route(Method::POST, "sync/link", Some(url)).await;
        assert_eq!((status, &body["slug"]), (200, &json!("abc123")));

        let (status, _) = fixture
            .route(Method::POST, "sync/pull", Some(json!({})))
            .await;
        assert_eq!(status, 401);
        let (status, body) = fixture.route(Method::POST, "sync/unlink", None).await;
        assert_eq!((status, &body["linked"]), (200, &json!(false)));

        for (method, path) in [
            (Method::GET, "sync/link"),
            (Method::POST, "sync"),
            (Method::GET, "sync/login/complete"),
            (Method::POST, "sync/other"),
        ] {
            assert_eq!(fixture.route(method, path, None).await.0, 404, "{path}");
        }
    }

    #[tokio::test]
    async fn keeping_a_project_on_another_site_is_refused_before_a_folder_is_asked_for() {
        let fixture = Fixture::new(PAGE).await;
        let request = json_request(json!({"url": "https://other.example/docs/abc123"}));
        let reply = handle_keep(&fixture.inner, &fixture.headers, fixture.origin(), request).await;
        assert_error(&read(reply).await, 400, "another site");
    }

    #[test]
    fn a_kept_folder_is_empty_or_a_git_repository() {
        let folder = tempfile::tempdir().unwrap();
        assert!(is_keepable_folder(folder.path()));
        std::fs::write(folder.path().join("main.tex"), "x").unwrap();
        assert!(!is_keepable_folder(folder.path()));
        std::fs::create_dir(folder.path().join(".git")).unwrap();
        assert!(is_keepable_folder(folder.path()));

        // A worktree's `.git` is a file.
        let worktree = tempfile::tempdir().unwrap();
        std::fs::write(worktree.path().join("main.tex"), "x").unwrap();
        std::fs::write(worktree.path().join(".git"), "gitdir: elsewhere\n").unwrap();
        assert!(is_keepable_folder(worktree.path()));
    }

    #[tokio::test]
    async fn a_folder_linked_to_another_project_is_refused_and_one_linked_to_this_is_pulled() {
        let fixture = Fixture::new(PAGE).await;
        fixture.link_to("https://paper.example/docs/abc123");

        let other = Link::parse("https://paper.example/docs/def456").unwrap();
        let reply = link_and_pull(
            &fixture.inner,
            fixture.binding.clone(),
            fixture.origin(),
            other,
        )
        .await;
        let (status, body) = read(reply).await;
        assert_eq!((status, &body["reason"]), (409, &json!("linked")), "{body}");
        assert_eq!(body["id"], json!(fixture.id()));
        assert_eq!(
            body["error"],
            json!("this repository is linked to another LibrePaper project")
        );

        // The same project is pulled, and the pull needs a sign-in.
        let same = Link::parse("https://paper.example/docs/abc123").unwrap();
        let reply = link_and_pull(
            &fixture.inner,
            fixture.binding.clone(),
            fixture.origin(),
            same,
        )
        .await;
        let (status, body) = read(reply).await;
        assert_eq!((status, &body["error"]), (401, &json!("sign in")), "{body}");
        assert_eq!(body["id"], json!(fixture.id()));
    }

    /// The site's device flow: the code is approved when `approved` is set.
    struct DeviceStub {
        approved: AtomicBool,
    }

    async fn device_start() -> Json<Value> {
        Json(json!({
            "device_code": "device-code",
            "user_code": "ABCD-EFGH",
            "verification_url": "https://paper.example/auth/device?code=ABCD-EFGH",
            "expires_in": 600,
            "interval": 5,
        }))
    }

    async fn device_token(
        axum::extract::State(stub): axum::extract::State<Arc<DeviceStub>>,
    ) -> (StatusCode, Json<Value>) {
        if stub.approved.load(Ordering::SeqCst) {
            (StatusCode::OK, Json(json!({"token": "lp_stub_token"})))
        } else {
            (
                StatusCode::BAD_REQUEST,
                Json(json!({"error": "authorization_pending"})),
            )
        }
    }

    async fn serve(app: axum::Router) -> (String, tokio::task::JoinHandle<()>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let task = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        (origin, task)
    }

    async fn device_site() -> (String, Arc<DeviceStub>, tokio::task::JoinHandle<()>) {
        let stub = Arc::new(DeviceStub {
            approved: AtomicBool::new(false),
        });
        let app = axum::Router::new()
            .route("/api/auth/device", post(device_start))
            .route("/api/auth/device/token", post(device_token))
            .with_state(stub.clone());
        let (origin, task) = serve(app).await;
        (origin, stub, task)
    }

    #[tokio::test]
    async fn signing_in_stores_the_token_once_the_code_is_approved() {
        let (site, stub, _task) = device_site().await;
        let fixture = Fixture::new(&site).await;
        let (status, begun) = fixture.login(json!({})).await;
        assert_eq!(status, 200, "{begun}");
        assert_eq!(begun["user_code"], "ABCD-EFGH");
        assert_eq!(
            begun["verification_url"],
            "https://paper.example/auth/device?code=ABCD-EFGH"
        );
        let authorization = begun["authorization_id"].as_str().unwrap().to_string();

        // The page has not approved the code yet: the page tries again.
        let waiting = fixture.complete_login(&authorization).await;
        assert_error(&waiting, 400, "approve the code");
        assert_eq!(
            credentials::stored_token_at(&fixture.inner.state_home, &site),
            ""
        );

        stub.approved.store(true, Ordering::SeqCst);
        let (status, done) = fixture.complete_login(&authorization).await;
        assert_eq!(status, 200, "{done}");
        assert_eq!(done["signed_in"], true);
        assert_eq!(done["linked"], false);
        assert_eq!(
            credentials::stored_token_at(&fixture.inner.state_home, &site),
            "lp_stub_token"
        );

        // A code is claimed once.
        let again = fixture.complete_login(&authorization).await;
        assert_error(&again, 400, "start again");
    }

    #[tokio::test]
    async fn a_sign_in_belongs_to_the_folder_that_started_it() {
        let (site, stub, _task) = device_site().await;
        stub.approved.store(true, Ordering::SeqCst);
        let fixture = Fixture::new(&site).await;
        let other = fixture.another_folder();
        let (_, begun) = fixture.login(json!({})).await;
        let authorization = begun["authorization_id"].as_str().unwrap().to_string();

        let stolen = other.complete_login(&authorization).await;
        assert_error(&stolen, 400, "does not belong");
        assert_eq!(
            credentials::stored_token_at(&fixture.inner.state_home, &site),
            ""
        );
        let (status, _) = fixture.complete_login(&authorization).await;
        assert_eq!(status, 200);
    }

    #[tokio::test]
    async fn a_newer_sign_in_replaces_the_one_before_it() {
        let (site, stub, _task) = device_site().await;
        stub.approved.store(true, Ordering::SeqCst);
        let fixture = Fixture::new(&site).await;
        let (_, first) = fixture.login(json!({})).await;
        let (_, second) = fixture.login(json!({})).await;
        let first = first["authorization_id"].as_str().unwrap().to_string();
        let second = second["authorization_id"].as_str().unwrap().to_string();

        assert_error(&fixture.complete_login(&first).await, 400, "start again");
        assert_eq!(fixture.complete_login(&second).await.0, 200);
    }

    #[tokio::test]
    async fn signing_in_names_the_pages_own_site_or_the_linked_one() {
        let (site, _stub, _task) = device_site().await;
        let fixture = Fixture::new(&site).await;
        let elsewhere = fixture
            .login(json!({"site": "https://other.example"}))
            .await;
        assert_error(&elsewhere, 400, "not the site of this page");
        let own = fixture.login(json!({"site": site})).await;
        assert_eq!(own.0, 200, "{}", own.1);
    }

    async fn rules_reply() -> Json<Value> {
        Json(json!({
            "text_extensions": ["tex", "typ", "md", "bib"],
            "asset_extensions": ["png", "jpg", "pdf"],
            "derived_extensions": ["pdf"],
            "max_path": 255,
            "max_files": 1000,
        }))
    }

    /// A site that serves the path rules and nothing else.
    async fn rules_site() -> (String, tokio::task::JoinHandle<()>) {
        serve(axum::Router::new().route("/api/config", get(rules_reply))).await
    }

    /// A site that answers everything with `status`, the path rules included
    /// unless `rules` serves them.
    async fn refusing_site(status: u16, rules: bool) -> (String, tokio::task::JoinHandle<()>) {
        let status = StatusCode::from_u16(status).unwrap();
        let mut app = axum::Router::new();
        if rules {
            app = app.route("/api/config", get(rules_reply));
        }
        let app = app.fallback(move || async move { (status, Json(json!({"error": "refused"}))) });
        serve(app).await
    }

    /// A site that counts the requests it gets.
    async fn counting_site() -> (String, Arc<AtomicUsize>, tokio::task::JoinHandle<()>) {
        let requests = Arc::new(AtomicUsize::new(0));
        let counter = requests.clone();
        let app = axum::Router::new().fallback(move || {
            let counter = counter.clone();
            async move {
                counter.fetch_add(1, Ordering::SeqCst);
                StatusCode::NOT_FOUND
            }
        });
        let (origin, task) = serve(app).await;
        (origin, requests, task)
    }

    #[tokio::test]
    async fn pushing_a_folder_that_matches_the_record_sends_nothing() {
        let (site, _task) = rules_site().await;
        let fixture = Fixture::new(&site).await;
        fixture.sign_in("lp_token");
        fixture.link_to(&format!("{site}/docs/abc123"));

        let (status, body) = fixture.push(json!({"message": "No changes"})).await;
        assert_eq!((status, body), (200, json!({"nothing": true})));
    }

    /// A site that serves the path rules and counts every other request.
    async fn counting_rules_site() -> (String, Arc<AtomicUsize>, tokio::task::JoinHandle<()>) {
        let requests = Arc::new(AtomicUsize::new(0));
        let counter = requests.clone();
        let app = axum::Router::new()
            .route("/api/config", get(rules_reply))
            .fallback(move || {
                let counter = counter.clone();
                async move {
                    counter.fetch_add(1, Ordering::SeqCst);
                    StatusCode::NOT_FOUND
                }
            });
        let (origin, task) = serve(app).await;
        (origin, requests, task)
    }

    /// The two files a local render leaves in a folder: its result, and a
    /// figure it made beside it.
    const RENDER_OUTPUTS: [&str; 2] = ["main.pdf", "main_files/figure-1.png"];

    fn write_render_outputs(fixture: &Fixture) {
        std::fs::write(fixture.root().join("main.pdf"), b"%PDF rendered").unwrap();
        std::fs::create_dir_all(fixture.root().join("main_files")).unwrap();
        std::fs::write(fixture.root().join("main_files/figure-1.png"), b"png").unwrap();
    }

    /// Records what the folder's render wrote, as the job worker does.
    fn note_render_outputs(fixture: &Fixture) {
        fixture
            .inner
            .projects
            .note_render_outputs(fixture.id(), RENDER_OUTPUTS.map(String::from));
    }

    #[tokio::test]
    async fn a_file_a_render_wrote_is_not_among_the_changes_a_push_sends() {
        let (site, requests, _task) = counting_rules_site().await;
        let fixture = Fixture::new(&site).await;
        fixture.sign_in("lp_token");
        fixture.link_to(&format!("{site}/docs/abc123"));
        write_render_outputs(&fixture);

        // Until the render is recorded the files are changes made here.
        let (_, body) = fixture.status().await;
        assert_eq!(body["changes"]["here"], 2, "{body}");

        note_render_outputs(&fixture);
        let (_, body) = fixture.status().await;
        assert_eq!(body["changes"]["here"], 0, "{body}");

        let before = requests.load(Ordering::SeqCst);
        let (status, body) = fixture.push(json!({"message": "Edits"})).await;
        assert_eq!((status, body), (200, json!({"nothing": true})));
        assert_eq!(
            requests.load(Ordering::SeqCst),
            before,
            "no figure was offered or uploaded"
        );
    }

    #[tokio::test]
    async fn a_file_a_render_wrote_is_not_published_with_the_folder() {
        let uploaded = Arc::new(std::sync::Mutex::new(String::new()));
        let held = uploaded.clone();
        let app = axum::Router::new()
            .route("/api/config", get(rules_reply))
            .route(
                "/api/documents",
                post(move |body: axum::body::Bytes| {
                    let held = held.clone();
                    async move {
                        *held.lock().unwrap() = String::from_utf8_lossy(&body).into_owned();
                        Json(json!({"slug": "abc123"}))
                    }
                }),
            );
        let (site, _task) = serve(app).await;
        let fixture = Fixture::new(&site).await;
        fixture.sign_in("lp_token");
        let plain = fixture.another_folder();
        for folder in [&fixture, &plain] {
            std::fs::write(folder.root().join("main.tex"), "main\n").unwrap();
            write_render_outputs(folder);
        }

        // A folder whose render is not recorded sends everything it holds.
        let (status, body) = plain.publish(json!({"title": "Paper"})).await;
        assert_eq!(status, 200, "{body}");
        let sent = uploaded.lock().unwrap().clone();
        assert!(sent.contains("filename=\"main.tex\""), "{sent}");
        assert!(sent.contains("filename=\"main.pdf\""), "{sent}");

        note_render_outputs(&fixture);
        let (status, body) = fixture.publish(json!({"title": "Paper"})).await;
        assert_eq!(status, 200, "{body}");
        let sent = uploaded.lock().unwrap().clone();
        assert!(sent.contains("filename=\"main.tex\""), "{sent}");
        assert!(!sent.contains("main.pdf"), "{sent}");
        assert!(!sent.contains("figure-1.png"), "{sent}");

        // Nor does it count them as changes made since.
        assert_eq!(body["changes"]["here"], 0, "{body}");
        let store = FileStore::for_dir(&fixture.inner.state_home, fixture.root());
        let state = store.load().unwrap().unwrap();
        assert_eq!(state.files.keys().collect::<Vec<_>>(), ["main.tex"]);
    }

    #[tokio::test]
    async fn a_pull_does_not_write_a_file_a_render_wrote() {
        let digest = "a".repeat(64);
        let version = json!({
            "files": {
                "main.tex": {"kind": "text"},
                "main.pdf": {"kind": "asset", "digest": digest},
                "main_files/figure-1.png": {"kind": "asset", "digest": digest},
            },
            "texts": {"main.tex": "main\n"},
        });
        let app = axum::Router::new()
            .route("/api/config", get(rules_reply))
            .fallback(move |uri: axum::http::Uri| {
                let version = version.clone();
                async move {
                    match uri.path() {
                        "/api/documents/abc123/history" => (
                            StatusCode::OK,
                            Json(json!({
                                "labels": [{"sha": "v1", "sequence": 1, "label": "First"}],
                                "next_cursor": null,
                            })),
                        ),
                        "/api/documents/abc123/history/v1" => (StatusCode::OK, Json(version)),
                        _ => (
                            StatusCode::NOT_FOUND,
                            Json(json!({"error": "no such file"})),
                        ),
                    }
                }
            });
        let (site, _task) = serve(app).await;
        let fixture = Fixture::new(&site).await;
        fixture.sign_in("lp_token");
        let plain = fixture.another_folder();
        for folder in [&fixture, &plain] {
            folder.link_to(&format!("{site}/docs/abc123"));
        }

        // Without the record the pull fetches the render's files from the site,
        // which does not hold them.
        let (status, body) = plain.pull(json!({})).await;
        assert_eq!(status, 502, "{body}");

        note_render_outputs(&fixture);
        let (status, body) = fixture.pull(json!({})).await;
        assert_eq!(status, 200, "{body}");
        assert_eq!(body["written"], json!(["main.tex"]));
        assert_eq!(body["skipped"], json!([]));
        assert!(fixture.root().join("main.tex").exists());
        assert!(!fixture.root().join("main.pdf").exists());
        assert!(!fixture.root().join("main_files").exists());
        let store = FileStore::for_dir(&fixture.inner.state_home, fixture.root());
        let state = store.load().unwrap().unwrap();
        assert_eq!(state.files.keys().collect::<Vec<_>>(), ["main.tex"]);
    }

    #[tokio::test]
    async fn a_folder_linked_from_another_site_is_not_the_pages_to_sync() {
        let (site_a, requests, _task) = counting_site().await;
        let fixture = Fixture::new(PAGE).await;
        // This computer holds a token for the other site, and the link names it.
        credentials::store_token_at(&fixture.inner.state_home, &site_a, "lp_token_a").unwrap();
        fixture.link_to(&format!("{site_a}/docs/abc123"));

        let pull = fixture.pull(json!({})).await;
        assert_error(&pull, 409, "another site");
        let push = fixture.push(json!({"message": "Edits"})).await;
        assert_error(&push, 409, "another site");
        let login = fixture.login(json!({})).await;
        assert_error(&login, 409, "another site");
        let publish = fixture.publish(json!({"title": "Paper"})).await;
        assert_error(&publish, 409, "another site");

        let (status, body) = fixture.status().await;
        assert_eq!(status, 200, "{body}");
        assert_eq!(body["linked"], false);
        assert_eq!(body["other_site"], true);
        assert_eq!(body["site"], Value::Null);
        assert_eq!(body["slug"], Value::Null);
        // The token is the other site's, so this page is not signed in with it.
        assert_eq!(body["signed_in"], false);
        assert!(body.get("error").is_none(), "{body}");

        // The other site was never asked anything, with its token or without.
        assert_eq!(requests.load(Ordering::SeqCst), 0);

        // The page can still take the folder over.
        let (status, body) = fixture.unlink().await;
        assert_eq!(status, 200, "{body}");
        assert!(body.get("other_site").is_none(), "{body}");
        let (_, body) = fixture.link("https://paper.example/docs/abc123").await;
        assert_eq!(body["linked"], true);
        assert!(body.get("other_site").is_none(), "{body}");
    }

    #[test]
    fn a_failed_call_answers_with_the_status_that_names_its_cause() {
        let http = |status| Error::Http {
            status,
            detail: "refused".to_string(),
        };
        let refused = (401, json!({"error": "sign in"}));
        assert_eq!(failed(&http(401)), refused);
        assert_eq!(failed(&http(403)), refused);
        for status in [400, 404, 409, 500] {
            let failure = failed(&http(status));
            assert_eq!(failure.0, 502, "{status}");
            assert_eq!(failure.1["error"], format!("refused ({status})"));
        }
        assert_eq!(
            failed(&Error::NotLinked),
            (
                409,
                json!({"error": "link this folder to a web project first"})
            )
        );
        assert_eq!(
            failed(&Error::Rules("too many files".to_string())),
            (400, json!({"error": "too many files"}))
        );
        assert_eq!(
            failed(&Error::Invalid("a push needs a message".to_string())),
            (400, json!({"error": "a push needs a message"}))
        );
        assert_eq!(failed(&Error::Link("not a project".to_string())).0, 400);
        assert_eq!(failed(&Error::Network("no route".to_string())).0, 502);
        assert_eq!(failed(&Error::Trimmed).0, 502);
        let io = Error::Io {
            path: PathBuf::from("main.tex"),
            source: std::io::Error::other("disk full"),
        };
        assert_eq!(failed(&io).0, 500);
        assert_eq!(failed(&Error::Git("no git".to_string())).0, 500);
        assert_eq!(failed(&Error::State("unreadable".to_string())).0, 500);
    }

    #[tokio::test]
    async fn a_site_that_refuses_the_token_asks_the_page_to_sign_in() {
        for status in [401, 403] {
            let (site, _task) = refusing_site(status, false).await;
            let fixture = Fixture::new(&site).await;
            fixture.sign_in("lp_revoked");
            fixture.link_to(&format!("{site}/docs/abc123"));

            let pull = fixture.pull(json!({})).await;
            assert_eq!((pull.0, &pull.1), (401, &json!({"error": "sign in"})));
            let push = fixture.push(json!({"message": "Edits"})).await;
            assert_eq!((push.0, &push.1), (401, &json!({"error": "sign in"})));
        }
    }

    #[tokio::test]
    async fn the_history_list_refusing_the_token_is_a_sign_in_too() {
        // The path rules are public; the version list is where the token counts.
        let (site, _task) = refusing_site(401, true).await;
        let fixture = Fixture::new(&site).await;
        fixture.sign_in("lp_revoked");
        fixture.link_to(&format!("{site}/docs/abc123"));

        let pull = fixture.pull(json!({})).await;
        assert_eq!((pull.0, &pull.1), (401, &json!({"error": "sign in"})));
    }

    #[tokio::test]
    async fn a_site_that_fails_is_a_bad_gateway_not_a_sign_in() {
        let (site, _task) = refusing_site(500, false).await;
        let fixture = Fixture::new(&site).await;
        fixture.sign_in("lp_token");
        fixture.link_to(&format!("{site}/docs/abc123"));

        let pull = fixture.pull(json!({})).await;
        assert_error(&pull, 502, "(500)");
        let push = fixture.push(json!({"message": "Edits"})).await;
        assert_error(&push, 502, "(500)");
    }

    #[tokio::test]
    async fn status_reads_a_refused_token_as_signed_out() {
        for status in [401, 403] {
            let (site, _task) = refusing_site(status, true).await;
            let fixture = Fixture::new(&site).await;
            fixture.sign_in("lp_revoked");
            fixture.link_to(&format!("{site}/docs/abc123"));

            let (code, body) = fixture.status().await;
            assert_eq!(code, 200, "{body}");
            assert_eq!(body["linked"], true);
            assert_eq!(body["signed_in"], false);
            assert_eq!(body["error"], "sign in");
        }

        // A site that merely fails leaves the sign-in standing.
        let (site, _task) = refusing_site(500, true).await;
        let fixture = Fixture::new(&site).await;
        fixture.sign_in("lp_token");
        fixture.link_to(&format!("{site}/docs/abc123"));
        let (_, body) = fixture.status().await;
        assert_eq!(body["signed_in"], true);
        assert!(body["error"].as_str().unwrap().contains("(500)"), "{body}");
    }
}
