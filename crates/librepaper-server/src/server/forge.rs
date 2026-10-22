//! Project sync with GitHub: the link row routes, the source-change route a
//! pull is applied through, and the relay that exchanges a code for a token.
//!
//! The browser does the GitHub work. This module keeps the record of what
//! the last sync left behind, applies a pull as one checked change, and
//! holds the client secret the browser cannot. It keeps no token and logs no
//! request or reply on the relay routes.

use super::*;
use librepaper_base::http::{provider_client, read_capped};
use librepaper_engine::storage::postgres::{ForgeLink, ForgeLinkWrite, PostgresCatalog};
use librepaper_room::room::agent::{AgentAuthority, OperationKey};
use librepaper_room::room::source_change::{
    AgentError, Consistency, CreateFile, DeleteFile, Dependency, Origin, Patch, PutBinary,
    SourceChange, TransactHook,
};

/// The cookie that ties a consent page's return to the redirect that began it.
const FORGE_STATE_COOKIE: &str = "librepaper_forge_state";
const FORGE_COOKIE_PATH: &str = "/api/forge/github";
const FORGE_TOKEN_MESSAGE: &str = "librepaper-forge-token";
/// The page's one data block, replaced whole with the message for the opener.
const TOKEN_PLACEHOLDER: &str = r#"<script type="application/json" id="forge-token">null</script>"#;

/// The GitHub App this deployment syncs through. The slug and the two hosts
/// are public and reach the browser through `/api/config`; the secret does
/// not.
#[derive(Clone)]
pub struct ForgeGithub {
    pub app_slug: String,
    pub client_id: String,
    pub client_secret: String,
    /// The API host the browser calls, `https://api.github.com` by default.
    pub api: String,
    /// The host of the consent page and the token endpoint.
    pub web: String,
}

impl std::fmt::Debug for ForgeGithub {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ForgeGithub")
            .field("app_slug", &self.app_slug)
            .field("client_id", &self.client_id)
            .field("client_secret", &"<redacted>")
            .field("api", &self.api)
            .field("web", &self.web)
            .finish()
    }
}

fn is_hex(value: &str, length: usize) -> bool {
    value.len() == length
        && value
            .bytes()
            .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
}

/// A path inside the linked folder or the project: no leading or trailing
/// slash, no empty, "." or ".." segment, no backslash or control character.
fn is_relative_path(path: &str) -> bool {
    !path.is_empty()
        && path.len() <= 4096
        && !path.contains('\\')
        && !path.chars().any(char::is_control)
        && path
            .split('/')
            .all(|segment| !segment.is_empty() && segment != "." && segment != "..")
}

fn is_repository(name: &str) -> bool {
    let Some((owner, repo)) = name.split_once('/') else {
        return false;
    };
    let part = |part: &str| {
        !part.is_empty()
            && part != "."
            && part != ".."
            && part.len() <= 100
            && part
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
    };
    part(owner) && part(repo)
}

fn is_branch(name: &str) -> bool {
    !name.is_empty() && name.len() <= 255 && !name.chars().any(char::is_control)
}

/// A map of synced paths to `{blob, hash}`, as the contract spells it.
fn check_map(value: &Value, max_files: usize) -> Result<(), String> {
    let Some(map) = value.as_object() else {
        return Err("a synced map must be an object".into());
    };
    if map.len() > max_files {
        return Err(format!("a synced map may hold at most {max_files} files"));
    }
    for (path, entry) in map {
        if !is_relative_path(path) {
            return Err(format!("{path:?} is not a valid synced path"));
        }
        let Some(fields) = entry.as_object() else {
            return Err(format!("the entry for {path:?} must be an object"));
        };
        let valid = fields.len() == 2
            && fields
                .get("blob")
                .and_then(Value::as_str)
                .is_some_and(|blob| is_hex(blob, 40))
            && fields
                .get("hash")
                .and_then(Value::as_str)
                .is_some_and(|hash| is_hex(hash, 64));
        if !valid {
            return Err(format!(
                "the entry for {path:?} needs a 40 digit blob and a 64 digit hash"
            ));
        }
    }
    Ok(())
}

fn check_export(value: &Value, max_files: usize) -> Result<(), String> {
    if value.is_null() {
        return Ok(());
    }
    let Some(fields) = value.as_object() else {
        return Err("an export must be null or an object".into());
    };
    let shaped = fields.len() == 3
        && fields
            .get("branch")
            .and_then(Value::as_str)
            .is_some_and(is_branch)
        && fields
            .get("commit")
            .and_then(Value::as_str)
            .is_some_and(|commit| is_hex(commit, 40));
    if !shaped {
        return Err("an export needs a branch, a 40 digit commit and a map".into());
    }
    check_map(&fields["map"], max_files)
}

/// Compares two short strings without stopping at the first difference.
fn same_text(left: &str, right: &str) -> bool {
    left.len() == right.len()
        && left
            .bytes()
            .zip(right.bytes())
            .fold(0_u8, |seen, (a, b)| seen | (a ^ b))
            == 0
}

fn no_content() -> Reply {
    let mut response = Response::new(Body::empty());
    *response.status_mut() = StatusCode::NO_CONTENT;
    response
}

fn stale_reply(link: Option<&Value>) -> Reply {
    write_json(
        409,
        &json!({"error": "the link changed", "reason": "stale", "link": link}),
    )
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PutLink {
    revision: Option<i64>,
    provider: String,
    repository_id: i64,
    repository: String,
    branch: String,
    folder: String,
    synced_commit: Option<String>,
    synced_map: Value,
    #[serde(default)]
    export: Option<Value>,
    #[serde(default)]
    pushed: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ChangeInput {
    #[serde(default)]
    dependencies: Vec<Dependency>,
    #[serde(default)]
    patches: Vec<Patch>,
    #[serde(default)]
    create: Vec<CreateFile>,
    #[serde(default)]
    delete: Vec<DeleteFile>,
    #[serde(default)]
    put_binary: Vec<PutBinary>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SyncInput {
    revision: i64,
    synced_commit: String,
    synced_map: Value,
    #[serde(default)]
    export_consumed: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SourceChangeInput {
    operation: String,
    provider: String,
    change: ChangeInput,
    sync: SyncInput,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RefreshInput {
    refresh_token: String,
}

impl Server {
    /// The document and its editor, or the reply that refuses. Reading the
    /// link needs the same access as writing it, because it names the
    /// repository the project is wired to.
    #[allow(clippy::result_large_err)]
    async fn forge_editor(
        &self,
        slug: &str,
        headers: &HeaderMap,
        arrival: &Arrival,
    ) -> Result<(Viewer, uuid::Uuid), Reply> {
        if !self.valid_slug(slug) {
            return Err(plain(400, "bad slug"));
        }
        let entry = match self.checked_entry(slug).await {
            Ok(Some(entry)) => entry,
            Ok(None) => return Err(no_such_document()),
            Err(response) => return Err(response),
        };
        let who = self.viewer(&entry, headers, arrival, None).await;
        if who.auth_failed {
            return Err(authentication_expired());
        }
        if self.needs_sign_in(&entry, &who) {
            return Err(sign_in_to_read());
        }
        if !self.may_read(&entry, &who) {
            return Err(no_such_document());
        }
        if !who.at_least(Role::Editor) {
            return Err(write_json(403, &json!({"error": "edit access required"})));
        }
        let Ok(document_id) = uuid::Uuid::parse_str(&entry.storage_id) else {
            return Err(write_json(500, &json!({"error": "the document has no id"})));
        };
        Ok((who, document_id))
    }

    /// The wire form of a link: the row without its document id, and the
    /// account that connected it as an id and a name.
    async fn forge_link_json(&self, link: &ForgeLink) -> Value {
        let connected_by = match link.connected_by {
            Some(id) => match self.store.catalog.account(id).await {
                Ok(Some(account)) => {
                    let name = if account.display_name.trim().is_empty() {
                        account.handle
                    } else {
                        account.display_name
                    };
                    json!({"id": id, "name": name})
                }
                _ => Value::Null,
            },
            None => Value::Null,
        };
        let mut value = serde_json::to_value(link).unwrap_or(Value::Null);
        if let Some(fields) = value.as_object_mut() {
            fields.remove("document_id");
            fields.insert("connected_by".into(), connected_by);
        }
        value
    }

    #[allow(clippy::result_large_err)]
    async fn forge_current_link(&self, document_id: uuid::Uuid) -> Result<Option<Value>, Reply> {
        match self.store.catalog.forge_link(document_id).await {
            Ok(Some(link)) => Ok(Some(self.forge_link_json(&link).await)),
            Ok(None) => Ok(None),
            Err(error) => Err(forge_storage_failure("read the link", &error)),
        }
    }

    /// `GET`, `PUT` and `DELETE /api/documents/{slug}/forge`.
    pub(super) async fn handle_forge_link(
        &self,
        request: Request<Body>,
        arrival: &Arrival,
        slug: &str,
    ) -> Reply {
        let method = request.method().clone();
        let headers = request.headers().clone();
        if method != Method::GET && cross_site_refused(&headers, arrival) {
            return write_json(403, &json!({"error": "cross-site request refused"}));
        }
        let (who, document_id) = match self.forge_editor(slug, &headers, arrival).await {
            Ok(found) => found,
            Err(response) => return response,
        };
        match method {
            Method::GET => match self.forge_current_link(document_id).await {
                Ok(link) => write_json(200, &json!({"link": link})),
                Err(response) => response,
            },
            Method::DELETE => match self.store.catalog.delete_forge_link(document_id).await {
                Ok(_) => no_content(),
                Err(error) => forge_storage_failure("forget the link", &error),
            },
            Method::PUT => {
                self.put_forge_link(request, arrival, slug, who.principal_key(), document_id)
                    .await
            }
            _ => plain(405, "method not allowed"),
        }
    }

    async fn put_forge_link(
        &self,
        request: Request<Body>,
        arrival: &Arrival,
        slug: &str,
        principal: String,
        document_id: uuid::Uuid,
    ) -> Reply {
        let headers = request.headers().clone();
        let cap = 64 * 1024 + 2 * self.config.max_files * 512;
        let body = match to_bytes(request.into_body(), cap).await {
            Ok(body) => body,
            Err(_) => return write_json(413, &json!({"error": "the link is too large"})),
        };
        // Access may have been withdrawn while the body was arriving.
        let who = match self.forge_editor(slug, &headers, arrival).await {
            Ok((who, _)) if who.principal_key() == principal => who,
            Ok(_) => return no_such_document(),
            Err(response) => return response,
        };
        let input: PutLink = match serde_json::from_slice(&body) {
            Ok(input) => input,
            Err(_) => return write_json(400, &json!({"error": "the link request is malformed"})),
        };
        let max_files = self.config.max_files;
        let invalid = |why: &str| write_json(400, &json!({"error": why}));
        if input.provider != "github" {
            return invalid("provider must be github");
        }
        if input.repository_id <= 0 {
            return invalid("repository_id must be positive");
        }
        if !is_repository(&input.repository) {
            return invalid("repository must be owner/name");
        }
        if !is_branch(&input.branch) {
            return invalid("branch must be 1 to 255 characters without control characters");
        }
        if !(input.folder.is_empty() || is_relative_path(&input.folder)) {
            return invalid("folder must be empty or a relative path without dot segments");
        }
        if input
            .synced_commit
            .as_deref()
            .is_some_and(|commit| !is_hex(commit, 40))
        {
            return invalid("synced_commit must be null or 40 lowercase hex digits");
        }
        if let Err(why) = check_map(&input.synced_map, max_files) {
            return invalid(&why);
        }
        let export = input.export.filter(|export| !export.is_null());
        if let Some(export) = &export {
            if let Err(why) = check_export(export, max_files) {
                return invalid(&why);
            }
        }
        if let Some(revision) = input.revision {
            if revision < 1 {
                return invalid("revision must be null or a stored revision");
            }
        }
        let write = ForgeLinkWrite {
            provider: input.provider,
            repository_id: input.repository_id,
            repository: input.repository,
            branch: input.branch,
            folder: input.folder,
            connected_by: who.account_id(),
            synced_commit: input.synced_commit,
            synced_map: input.synced_map,
            export,
            pulled: false,
            pushed: input.pushed,
        };
        match self
            .store
            .catalog
            .put_forge_link_now(document_id, input.revision, &write)
            .await
        {
            Ok(Some(link)) => write_json(200, &json!({"link": self.forge_link_json(&link).await})),
            Ok(None) => {
                let current = match self.forge_current_link(document_id).await {
                    Ok(current) => current,
                    Err(response) => return response,
                };
                write_json(409, &json!({"error": "the link changed", "link": current}))
            }
            Err(error) => forge_storage_failure("save the link", &error),
        }
    }

    /// `POST /api/documents/{slug}/source-change`: one checked change to the
    /// project, with the sync record written in the same transaction.
    pub(super) async fn handle_source_change(
        &self,
        request: Request<Body>,
        arrival: &Arrival,
        slug: &str,
    ) -> Reply {
        let headers = request.headers().clone();
        if cross_site_refused(&headers, arrival) {
            return write_json(403, &json!({"error": "cross-site request refused"}));
        }
        let (first, document_id) = match self.forge_editor(slug, &headers, arrival).await {
            Ok(found) => found,
            Err(response) => return response,
        };
        // The deployment's log quota bounds a pull's text and its sync map
        // together, the same ceiling a socket update has.
        let body = match to_bytes(request.into_body(), self.config.log_quota_bytes).await {
            Ok(body) => body,
            Err(_) => return write_json(413, &json!({"error": "the change is too large"})),
        };
        let principal = first.principal_key();
        let who = match self.forge_editor(slug, &headers, arrival).await {
            Ok((who, _)) if who.principal_key() == principal => who,
            Ok(_) => return no_such_document(),
            Err(response) => return response,
        };
        let input: SourceChangeInput = match serde_json::from_slice(&body) {
            Ok(input) => input,
            Err(_) => return write_json(400, &json!({"error": "the change is malformed"})),
        };
        let invalid = |why: &str| write_json(400, &json!({"error": why}));
        if input.provider != "github" {
            return invalid("provider must be github");
        }
        if !is_hex(&input.sync.synced_commit, 40) {
            return invalid("sync.synced_commit must be 40 lowercase hex digits");
        }
        if let Err(why) = check_map(&input.sync.synced_map, self.config.max_files) {
            return invalid(&why);
        }
        let row = match self.store.catalog.forge_link(document_id).await {
            Ok(Some(row)) => row,
            Ok(None) => return stale_reply(None),
            Err(error) => return forge_storage_failure("read the link", &error),
        };
        // A revision that moved is not refused here. A retry of a change that
        // already landed must answer its first receipt, and the revision it
        // carried is behind by then. The hook below enforces the revision
        // inside the transaction, so a stale change is rolled back whole.
        let room = match self.rooms.get(slug).await {
            Ok(room) => room,
            Err(error) => return refused("open the room", &error),
        };
        let change = SourceChange {
            operation: OperationKey {
                epoch: "forge".into(),
                id: input.operation,
            },
            base_tree: String::new(),
            consistency: Consistency::UnchangedDependencies,
            dependencies: input.change.dependencies,
            patches: input.change.patches,
            create: input.change.create,
            delete: input.change.delete,
            put_binary: input.change.put_binary,
            request_digest: hex::encode(Sha256::digest(&body)),
        };
        let ceiling = self.ceiling_for(&who.id);
        let authority = AgentAuthority {
            account_id: who.id.id.clone(),
            owner_key: who.key.clone(),
            session_generation: who.id.session_generation.clone(),
            link_hash: who.link.clone(),
            policy_editor: ceiling.edit,
            policy_comment: ceiling.comment,
            operation_scope: format!("forge:{document_id}:{principal}"),
        };
        let write = ForgeLinkWrite {
            provider: row.provider.clone(),
            repository_id: row.repository_id,
            repository: row.repository.clone(),
            branch: row.branch.clone(),
            folder: row.folder.clone(),
            connected_by: None,
            synced_commit: Some(input.sync.synced_commit),
            synced_map: input.sync.synced_map,
            export: if input.sync.export_consumed {
                None
            } else {
                row.export.clone()
            },
            pulled: true,
            pushed: false,
        };
        let revision = input.sync.revision;
        let hook: TransactHook = Box::new(move |connection| {
            Box::pin(async move {
                match PostgresCatalog::put_forge_link(
                    connection,
                    document_id,
                    Some(revision),
                    &write,
                )
                .await
                {
                    Ok(Some(_)) => Ok(()),
                    Ok(None) => Err("stale".to_string()),
                    Err(error) => Err(format!("storage: {error}")),
                }
            })
        });
        let recheck = || async {
            match self.forge_editor(slug, &headers, arrival).await {
                Ok((now, _)) if now.principal_key() == principal => Ok(()),
                _ => Err(AgentError::Conflict("document access changed".into())),
            }
        };
        let applied = room
            .apply_source_change(
                change,
                Origin::Sync {
                    provider: "github".into(),
                },
                authority,
                None,
                recheck,
                Some(hook),
            )
            .await;
        let receipt = match applied {
            Ok(receipt) => receipt,
            Err(error) => return self.source_change_refusal(document_id, error).await,
        };
        let link = match self.forge_current_link(document_id).await {
            Ok(link) => link,
            Err(response) => return response,
        };
        write_json(200, &json!({"receipt": receipt, "link": link}))
    }

    async fn source_change_refusal(&self, document_id: uuid::Uuid, error: AgentError) -> Reply {
        match error {
            AgentError::Hook(why) if why == "stale" => {
                match self.forge_current_link(document_id).await {
                    Ok(link) => stale_reply(link.as_ref()),
                    Err(response) => response,
                }
            }
            AgentError::Hook(why) => {
                tracing::warn!("source change could not record the sync: {why}");
                write_json(
                    503,
                    &json!({"error": "could not record the sync", "retryable": true}),
                )
            }
            AgentError::Changed(files) => write_json(
                409,
                &json!({
                    "error": "files changed since the change was planned",
                    "reason": "changed",
                    "files": files,
                }),
            ),
            AgentError::Suggestion { path, change } => write_json(
                409,
                &json!({
                    "error": "a pending suggestion is in the way",
                    "reason": "suggestion",
                    "path": path,
                    "change": change,
                }),
            ),
            AgentError::Invalid(why) => write_json(400, &json!({"error": why})),
            AgentError::Conflict(why) => {
                write_json(409, &json!({"error": why, "reason": "conflict"}))
            }
            AgentError::Storage(why) => {
                tracing::warn!("source change failed in storage: {why}");
                write_json(
                    503,
                    &json!({"error": "could not apply the change", "retryable": true}),
                )
            }
        }
    }

    /// Only a signed-in account may use the relay, and a failed lookup is
    /// not read as an anonymous caller.
    #[allow(clippy::result_large_err)]
    async fn forge_signed_in(&self, headers: &HeaderMap, arrival: &Arrival) -> Result<(), Reply> {
        match self.authenticated_identity(headers, arrival).await {
            Ok(who) if who.is_signed_in() => Ok(()),
            Err(AuthenticationFailure::Unavailable) => Err(write_json(
                503,
                &json!({"error": "authentication service temporarily unavailable"}),
            )),
            _ => Err(write_json(
                401,
                &json!({"error": "sign in to connect GitHub"}),
            )),
        }
    }

    /// `GET /api/forge/github/authorize`: sends a signed-in person to GitHub's
    /// consent page with a fresh state, kept in a short-lived cookie.
    pub(super) async fn handle_forge_authorize(
        &self,
        request: Request<Body>,
        arrival: &Arrival,
    ) -> Reply {
        let Some(forge) = &self.forge_github else {
            return plain(404, "not found");
        };
        if let Err(response) = self.forge_signed_in(request.headers(), arrival).await {
            return response;
        }
        let state = random_token();
        let Ok(mut target) = url::Url::parse(&format!("{}/login/oauth/authorize", forge.web))
        else {
            return plain(500, "the GitHub host is not a valid URL");
        };
        target
            .query_pairs_mut()
            .append_pair("client_id", &forge.client_id)
            .append_pair("state", &state)
            .append_pair("redirect_uri", &arrival.forge_callback_url());
        let mut response = redirect(target.as_str());
        add_cookie(
            &mut response,
            &forge_state_cookie(&state, 600, arrival.is_https()),
        );
        set(&mut response, "cache-control", "no-store");
        response
    }

    /// `GET /api/forge/github/callback`: checks the state, trades the code for
    /// a token and hands the token to the opening window. Nothing is stored.
    pub(super) async fn handle_forge_callback(
        &self,
        request: Request<Body>,
        arrival: &Arrival,
    ) -> Reply {
        let Some(forge) = &self.forge_github else {
            return plain(404, "not found");
        };
        let headers = request.headers();
        let query: HashMap<String, String> = request
            .uri()
            .query()
            .map(|query| {
                url::form_urlencoded::parse(query.as_bytes())
                    .into_owned()
                    .collect()
            })
            .unwrap_or_default();
        let expected = cookie(headers, FORGE_STATE_COOKIE).unwrap_or_default();
        let offered = query.get("state").map(String::as_str).unwrap_or_default();
        let message = if expected.is_empty() || !same_text(&expected, offered) {
            forge_error_message("The sign-in did not match. Close this window and try again.")
        } else if query.contains_key("error") {
            forge_error_message("GitHub did not allow the connection.")
        } else {
            match query.get("code").filter(|code| !code.is_empty()) {
                None => {
                    forge_error_message("GitHub sent no code. Close this window and try again.")
                }
                Some(code) => match self
                    .github_token(
                        forge,
                        json!({
                            "client_id": forge.client_id,
                            "client_secret": forge.client_secret,
                            "code": code,
                            "redirect_uri": arrival.forge_callback_url(),
                        }),
                    )
                    .await
                {
                    Ok(grant) => {
                        let mut message = json!({
                            "type": FORGE_TOKEN_MESSAGE,
                            "provider": "github",
                        });
                        if let (Some(fields), Some(grant)) =
                            (message.as_object_mut(), grant.as_object())
                        {
                            fields.extend(grant.clone());
                        }
                        message
                    }
                    Err((_, why)) => forge_error_message(why),
                },
            }
        };
        let mut response = self.forge_callback_page(headers, &message);
        add_cookie(&mut response, &clear_forge_state_cookie(arrival.is_https()));
        response
    }

    /// The callback page with the message in its data block. The shell page
    /// is served as every shell page is; only the block changes.
    fn forge_callback_page(&self, headers: &HeaderMap, message: &Value) -> Reply {
        let Some(asset) = self.shell.get("/forge-callback.html") else {
            return plain(404, "not found");
        };
        let page = String::from_utf8_lossy(&asset.body);
        if !page.contains(TOKEN_PLACEHOLDER) {
            return plain(500, "the callback page has no token block");
        }
        let data = serde_json::to_string(message)
            .unwrap_or_else(|_| "null".into())
            .replace('<', "\\u003c");
        let block = format!(r#"<script type="application/json" id="forge-token">{data}</script>"#);
        let file = ShellFile {
            body: Bytes::from(page.replacen(TOKEN_PLACEHOLDER, &block, 1)),
            brotli: None,
            immutable: false,
            ..asset.clone()
        };
        let mut response = write_asset(&file, headers);
        set(&mut response, "cache-control", "no-store");
        set(&mut response, "referrer-policy", "no-referrer");
        response
    }

    /// `POST /api/forge/github/refresh`: relays a refresh token to GitHub with
    /// the client secret and returns the new grant. Nothing is stored.
    pub(super) async fn handle_forge_refresh(
        &self,
        request: Request<Body>,
        arrival: &Arrival,
    ) -> Reply {
        let Some(forge) = &self.forge_github else {
            return plain(404, "not found");
        };
        let headers = request.headers().clone();
        if cross_site_refused(&headers, arrival) {
            return write_json(403, &json!({"error": "cross-site request refused"}));
        }
        if let Err(response) = self.forge_signed_in(&headers, arrival).await {
            return response;
        }
        let body = match to_bytes(request.into_body(), 4096).await {
            Ok(body) => body,
            Err(_) => return write_json(413, &json!({"error": "the request is too large"})),
        };
        let input: RefreshInput = match serde_json::from_slice(&body) {
            Ok(input) => input,
            Err(_) => return write_json(400, &json!({"error": "refresh_token is required"})),
        };
        if input.refresh_token.trim().is_empty() {
            return write_json(400, &json!({"error": "refresh_token is required"}));
        }
        let grant = self
            .github_token(
                forge,
                json!({
                    "grant_type": "refresh_token",
                    "client_id": forge.client_id,
                    "client_secret": forge.client_secret,
                    "refresh_token": input.refresh_token,
                }),
            )
            .await;
        let mut response = match grant {
            Ok(grant) => write_json(200, &grant),
            Err((status, why)) => write_json(status, &json!({"error": why})),
        };
        set(&mut response, "cache-control", "no-store");
        response
    }

    /// One call to GitHub's token endpoint. The answer is the four fields the
    /// browser keeps, or a status and a sentence that carries nothing GitHub
    /// said. Neither the request nor the reply is logged.
    async fn github_token(
        &self,
        forge: &ForgeGithub,
        form: Value,
    ) -> Result<Value, (u16, &'static str)> {
        let Some(_permit) = librepaper_base::auth::try_provider_request() else {
            return Err((503, "The connection service is busy. Try again shortly."));
        };
        let response = provider_client()
            .post(format!("{}/login/oauth/access_token", forge.web))
            .timeout(Duration::from_secs(15))
            .header("accept", "application/json")
            .json(&form)
            .send()
            .await
            .map_err(|_| (502, "GitHub could not be reached. Try again shortly."))?;
        let (_, reply) = read_capped(response)
            .await
            .map_err(|_| (502, "GitHub sent an unreadable reply. Try again shortly."))?;
        let reply: Value = serde_json::from_slice(&reply)
            .map_err(|_| (502, "GitHub sent an unreadable reply. Try again shortly."))?;
        let token = reply
            .get("access_token")
            .and_then(Value::as_str)
            .filter(|token| !token.is_empty());
        if token.is_none() || reply.get("error").is_some() {
            return Err((401, "GitHub did not accept the sign-in."));
        }
        let field = |name: &str| reply.get(name).cloned().unwrap_or(Value::Null);
        Ok(json!({
            "access_token": field("access_token"),
            "expires_in": field("expires_in"),
            "refresh_token": field("refresh_token"),
            "refresh_token_expires_in": field("refresh_token_expires_in"),
        }))
    }
}

fn forge_error_message(why: &str) -> Value {
    json!({"type": FORGE_TOKEN_MESSAGE, "error": why})
}

fn forge_state_cookie(value: &str, max_age: i64, https: bool) -> String {
    let mut cookie = format!(
        "{FORGE_STATE_COOKIE}={value}; Path={FORGE_COOKIE_PATH}; Max-Age={max_age}; HttpOnly; SameSite=Lax"
    );
    if https {
        cookie.push_str("; Secure");
    }
    cookie
}

fn clear_forge_state_cookie(https: bool) -> String {
    forge_state_cookie("", 0, https)
}

/// A storage failure answers a retryable 503 and logs what failed, never
/// anything the caller sent.
fn forge_storage_failure(what: &str, error: &dyn std::fmt::Display) -> Reply {
    tracing::warn!("could not {what}: {error}");
    write_json(
        503,
        &json!({"error": format!("could not {what}"), "retryable": true}),
    )
}

#[cfg(test)]
#[path = "forge_tests.rs"]
mod tests;
