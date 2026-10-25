//! Sync routes on a folder project: keep a folder in step with a web project.
//!
//! The work is `librepaper_sync`'s, the same pull and push the `librepaper`
//! command line runs, in the form for a folder that is not a Git repository.
//! This file authenticates, finds the folder, holds the sign-in the device
//! flow ends with, and shapes the answers. The link and the hashes of what
//! was last pulled or pushed live in the state home keyed by the folder, never
//! in the folder.
//!
//! A folder may only be linked to a project on the site of the page that asks,
//! so a page cannot spend the token this computer holds for another site.
//! Pull and push touch no change-poller state: the files a pull writes are
//! outside changes like any other, and the poll hands them to the editor.

use super::*;

use librepaper_base::http::Credentials;
use librepaper_sync::{
    changes_here, publish, pull, push, web_changed, Client, Error, FileStore, Link, PullOptions,
    PullOutcome, PushOptions, PushOutcome, State, StateStore,
};

use crate::local::bindings::ProjectBinding;
use crate::local::credentials;

/// How long a code waits to be approved, and how many wait at once.
const LOGIN_TTL: Duration = Duration::from_secs(10 * 60);
const MAX_PENDING_LOGINS: usize = 32;
/// The longest a call to the site's sign-in endpoints may take.
const LOGIN_TIMEOUT: Duration = Duration::from_secs(10);
const MAX_DEVICE_FIELD_BYTES: usize = 4096;

/// A refusal: the status and the JSON body to answer with.
type Failure = (u16, Value);

fn refuse(status: u16, error: &str) -> Failure {
    (status, json!({"error": error}))
}

/// A failed call into the sync crate. The crate's message already says what
/// went wrong in plain words.
fn failed(error: &Error) -> Failure {
    (502, json!({"error": error.to_string()}))
}

fn answer(result: Result<Value, Failure>) -> Reply {
    match result {
        Ok(body) => write_json(200, &body),
        Err((status, body)) => write_json(status, &body),
    }
}

/// The folder behind a sync route, with the store that holds its link.
struct Folder {
    binding: ProjectBinding,
    store: FileStore,
}

impl Folder {
    fn new(inner: &Inner, binding: ProjectBinding) -> Folder {
        let store = FileStore::for_dir(&inner.state_home, &binding.root);
        Folder { binding, store }
    }

    fn state(&self) -> Result<Option<State>, Failure> {
        self.store.load().map_err(|error| failed(&error))
    }

    fn linked(&self) -> Result<State, Failure> {
        self.state()?
            .ok_or_else(|| refuse(409, "link this folder to a web project first"))
    }

    /// A folder that is a Git repository syncs through Git. Pull, push and
    /// publish here are the form for a folder without it.
    fn refuse_git(&self) -> Result<(), Failure> {
        if is_git_folder(&self.binding.root) {
            return Err(refuse(
                409,
                "this folder is a Git repository; sync it with Git in a terminal",
            ));
        }
        Ok(())
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
fn origin_site(origin: Option<&str>) -> Result<url::Url, Failure> {
    url::Url::parse(&store::normalize_origin(origin.unwrap_or_default()))
        .map_err(|_| refuse(400, "missing or invalid X-LibrePaper-Origin header"))
}

fn same_site(left: &url::Url, right: &url::Url) -> bool {
    store::normalize_origin(left.as_str()) == store::normalize_origin(right.as_str())
}

/// A project link that names the requesting page's own site.
fn parse_link(raw: &str, origin: Option<&str>) -> Result<Link, Failure> {
    let link = Link::parse(raw.trim()).map_err(|error| refuse(400, &error.to_string()))?;
    if !same_site(&link.site, &origin_site(origin)?) {
        return Err(refuse(400, "that project is on another site"));
    }
    Ok(link)
}

fn project_url(link: &Link) -> String {
    format!("{}/docs/{}", site_text(&link.site), link.slug)
}

/// A client for `site` with the token this computer holds for it, or the 401
/// that asks the page to sign in.
fn client_for(inner: &Inner, site: &url::Url) -> Result<Client, Failure> {
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
/// the folder can still be unlinked.
async fn folder_status(
    inner: &Inner,
    folder: &Folder,
    origin: Option<&str>,
) -> Result<Value, Failure> {
    let state = folder.state()?.map(Arc::new);
    let site = match &state {
        Some(state) => state.link.site.clone(),
        None => origin_site(origin)?,
    };
    let token = credentials::stored_token_at(&inner.state_home, site.as_str());
    let signed_in = !token.is_empty();
    let (mut here, mut web, mut problem) = (0, false, None);
    if let (Some(state), true) = (&state, signed_in) {
        let client = Client::new(site.clone(), Credentials::token(&token));
        let (rules, moved) = tokio::join!(client.rules(), web_changed(&client, state));
        match moved {
            Ok(moved) => web = moved,
            Err(error) => problem = Some(error.to_string()),
        }
        match rules {
            Ok(rules) => {
                let (root, state) = (folder.binding.root.clone(), state.clone());
                match tokio::task::spawn_blocking(move || changes_here(&root, &state, &rules)).await
                {
                    Ok(Ok(count)) => here = count,
                    Ok(Err(error)) => problem = Some(error.to_string()),
                    Err(_) => problem = Some("the folder read stopped".to_string()),
                }
            }
            Err(error) => problem = Some(error.to_string()),
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
    binding: String,
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
    answer(begin_login(inner, &folder, origin, body.site).await)
}

/// Asks the site for a code. A linked folder signs in to its project's site;
/// an unlinked one to the page's own, or to `site` when that names it.
async fn begin_login(
    inner: &Inner,
    folder: &Folder,
    origin: Option<&str>,
    site: Option<String>,
) -> Result<Value, Failure> {
    let page = origin_site(origin)?;
    let site = match (folder.state()?, site) {
        (Some(state), _) => state.link.site,
        (None, None) => page,
        (None, Some(raw)) => {
            let site =
                url::Url::parse(raw.trim()).map_err(|_| refuse(400, "that site is not a URL"))?;
            if !same_site(&site, &page) {
                return Err(refuse(400, "that is not the site of this page"));
            }
            site
        }
    };
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
        // A newer request for this folder replaces the one before it.
        pending.retain(|_, login| login.binding != folder.binding.id);
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
                binding: folder.binding.id.clone(),
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
    answer(finish_login(inner, &folder, origin, &body.authorization_id).await)
}

/// Claims the token once the page has approved the code, stores it under the
/// site's origin as the backups and the command line do, and answers the
/// folder's status. A code not yet approved is a 400 the page retries.
async fn finish_login(
    inner: &Inner,
    folder: &Folder,
    origin: Option<&str>,
    authorization_id: &str,
) -> Result<Value, Failure> {
    let (site, device_code) = {
        let pending = inner.sync_logins.live();
        let Some(login) = pending.get(authorization_id) else {
            return Err(refuse(
                400,
                "the sign-in expired or was replaced; start again",
            ));
        };
        if login.origin != store::normalize_origin(origin.unwrap_or_default())
            || login.binding != folder.binding.id
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
    folder_status(inner, folder, origin).await
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
    folder.refuse_git()?;
    if title.is_empty() {
        return Err(refuse(400, "a project needs a title"));
    }
    if folder.state()?.is_some() {
        return Err(refuse(
            409,
            "this folder is already linked to a web project",
        ));
    }
    let client = client_for(inner, &origin_site(origin)?)?;
    let link = publish(&client, &folder.binding.root, &folder.store, title)
        .await
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
    answer(run_pull(inner, &folder, mine, theirs).await)
}

/// Takes the web project's newest version into the folder. A file changed on
/// both sides is not written; `mine` and `theirs` settle those, per path.
async fn run_pull(
    inner: &Inner,
    folder: &Folder,
    mine: Vec<String>,
    theirs: Vec<String>,
) -> Result<Value, Failure> {
    folder.refuse_git()?;
    let state = folder.linked()?;
    let client = client_for(inner, &state.link.site)?;
    let options = PullOptions {
        merge: false,
        mine,
        theirs,
    };
    let outcome = pull(&client, &folder.binding.root, &folder.store, &options)
        .await
        .map_err(|error| failed(&error))?;
    match outcome {
        PullOutcome::Folder {
            written,
            trashed,
            kept,
            conflicts,
            trimmed,
        } => Ok(json!({
            "written": written,
            "trashed": trashed,
            "kept": kept,
            "conflicts": conflicts,
            "trimmed": trimmed,
        })),
        PullOutcome::UpToDate => Ok(json!({
            "written": [],
            "trashed": [],
            "kept": [],
            "conflicts": [],
            "trimmed": false,
        })),
        _ => Err(refuse(
            500,
            "the pull ended in a way this app does not handle",
        )),
    }
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
    answer(run_push(inner, &folder, body.message.trim()).await)
}

/// Sends what changed in the folder as one version. The page has saved its
/// edits to the folder first, so the files are what the editor holds.
async fn run_push(inner: &Inner, folder: &Folder, message: &str) -> Result<Value, Failure> {
    folder.refuse_git()?;
    if message.is_empty() {
        return Err(refuse(400, "a push needs a message"));
    }
    let state = folder.linked()?;
    let client = client_for(inner, &state.link.site)?;
    let options = PushOptions {
        message: message.to_owned(),
        author: None,
    };
    let outcome = push(&client, &folder.binding.root, &folder.store, &options)
        .await
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

/// Keeps a web project in a new folder: asks for an empty one, opens it as a
/// folder project, links it and pulls. Once the folder is opened, a failure
/// still answers with the folder's `id`, so the page can open it and try again
/// from its Sync section, or sign in first.
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
    let root = match projects::choose_root(inner).await {
        Ok(root) => root,
        Err(response) => return response,
    };
    if !is_empty_dir(&root) {
        return write_json(409, &json!({"error": "choose an empty folder"}));
    }
    let binding = match inner
        .bindings
        .open_project(origin.unwrap_or_default(), &root)
    {
        Ok(binding) => binding,
        Err(error) => return write_json(400, &json!({"error": error})),
    };
    let mut opened = projects::opened(&binding);
    let folder = Folder::new(inner, binding);
    let pulled = async {
        record_link(&folder, link)?;
        run_pull(inner, &folder, Vec::new(), Vec::new()).await
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

#[cfg(test)]
mod tests {
    use super::*;

    use std::sync::atomic::{AtomicBool, Ordering};

    use axum::routing::{get, post};
    use axum::Json;

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

    #[tokio::test]
    async fn a_git_repository_is_left_to_git() {
        let fixture = Fixture::new(PAGE).await;
        std::fs::create_dir(fixture.root().join(".git")).unwrap();
        fixture.link_to("https://paper.example/docs/abc123");

        let (_, body) = fixture.status().await;
        assert_eq!(body["git"], true);
        let pull = fixture.pull(json!({})).await;
        assert_error(&pull, 409, "Git");
        let push = fixture.push(json!({"message": "Edits"})).await;
        assert_error(&push, 409, "Git");
        let publish = fixture.publish(json!({"title": "Paper"})).await;
        assert_error(&publish, 409, "Git");
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

    /// A site that serves the path rules and nothing else.
    async fn rules_site() -> (String, tokio::task::JoinHandle<()>) {
        let app = axum::Router::new().route(
            "/api/config",
            get(|| async {
                Json(json!({
                    "text_extensions": ["tex", "typ", "md", "bib"],
                    "asset_extensions": ["png", "jpg"],
                    "derived_extensions": ["pdf"],
                    "max_path": 255,
                    "max_files": 1000,
                }))
            }),
        );
        serve(app).await
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
}
