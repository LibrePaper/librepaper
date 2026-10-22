//! Project sync: the link row routes and the source-change route a pull is
//! applied through.
//!
//! The desktop does the git work. This module keeps the record of what the
//! last sync left behind and applies a pull as one checked change. The remote
//! URL and credentials never reach it, only a label.

use super::*;
use librepaper_engine::storage::postgres::{PostgresCatalog, SyncLink, SyncLinkWrite};
use librepaper_room::room::agent::{AgentAuthority, OperationKey};
use librepaper_room::room::source_change::{
    AgentError, Consistency, CreateFile, DeleteFile, Dependency, Origin, Patch, PutBinary,
    SourceChange, TransactHook,
};

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

fn is_branch(name: &str) -> bool {
    !name.is_empty() && name.len() <= 255 && !name.chars().any(char::is_control)
}

/// The synced paths of a request: valid project paths, no repeats, at most
/// `max_files` of them.
fn check_paths(paths: &[String], max_files: usize) -> Result<(), String> {
    if paths.len() > max_files {
        return Err(format!("synced_paths may hold at most {max_files} files"));
    }
    let mut seen = std::collections::HashSet::new();
    for path in paths {
        if !is_relative_path(path) {
            return Err(format!("{path:?} is not a valid synced path"));
        }
        if !seen.insert(path.as_str()) {
            return Err(format!("{path:?} appears twice in synced_paths"));
        }
    }
    Ok(())
}

/// A remote kind is `network` or `local`.
fn is_remote_kind(kind: &str) -> bool {
    matches!(kind, "network" | "local")
}

/// A label is short plain text and never carries credentials: a URL with
/// user info before the host is refused.
fn check_remote_label(label: &str) -> Result<(), String> {
    if label.is_empty() || label.len() > 200 || label.chars().any(char::is_control) {
        return Err("remote_label must be 1 to 200 characters without control characters".into());
    }
    if let Some((scheme, rest)) = label.split_once("://") {
        let host = rest.split('/').next().unwrap_or_default();
        let plain_scheme = !scheme.is_empty()
            && scheme
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte == b'+');
        if plain_scheme && host.contains('@') {
            return Err("remote_label must not carry user info".into());
        }
    }
    Ok(())
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
    remote_kind: String,
    remote_label: String,
    connection_id: uuid::Uuid,
    branch: String,
    folder: String,
    checkpoint: Option<String>,
    synced_paths: Vec<String>,
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
    checkpoint: String,
    synced_paths: Vec<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SourceChangeInput {
    operation: String,
    change: ChangeInput,
    sync: SyncInput,
}

impl Server {
    /// The document and its editor, or the reply that refuses. Reading the
    /// link needs the same access as writing it, because it names the
    /// repository the project is wired to.
    #[allow(clippy::result_large_err)]
    async fn sync_editor(
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

    /// An account as an id and a name, or null if it is gone.
    async fn sync_person(&self, id: Option<uuid::Uuid>) -> Value {
        let Some(id) = id else {
            return Value::Null;
        };
        match self.store.catalog.account(id).await {
            Ok(Some(account)) => {
                let name = if account.display_name.trim().is_empty() {
                    account.handle
                } else {
                    account.display_name
                };
                json!({"id": id, "name": name})
            }
            _ => Value::Null,
        }
    }

    /// The wire form of a link: the row without its document id, and the
    /// accounts that connected, pulled and pushed as an id and a name.
    async fn sync_link_json(&self, link: &SyncLink) -> Value {
        let connected_by = self.sync_person(link.connected_by).await;
        let last_pull_by = self.sync_person(link.last_pull_by).await;
        let last_push_by = self.sync_person(link.last_push_by).await;
        let mut value = serde_json::to_value(link).unwrap_or(Value::Null);
        if let Some(fields) = value.as_object_mut() {
            fields.remove("document_id");
            fields.insert("connected_by".into(), connected_by);
            fields.insert("last_pull_by".into(), last_pull_by);
            fields.insert("last_push_by".into(), last_push_by);
        }
        value
    }

    #[allow(clippy::result_large_err)]
    async fn sync_current_link(&self, document_id: uuid::Uuid) -> Result<Option<Value>, Reply> {
        match self.store.catalog.sync_link(document_id).await {
            Ok(Some(link)) => Ok(Some(self.sync_link_json(&link).await)),
            Ok(None) => Ok(None),
            Err(error) => Err(sync_storage_failure("read the link", &error)),
        }
    }

    /// `GET`, `PUT` and `DELETE /api/documents/{slug}/sync`.
    pub(super) async fn handle_sync_link(
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
        let (who, document_id) = match self.sync_editor(slug, &headers, arrival).await {
            Ok(found) => found,
            Err(response) => return response,
        };
        match method {
            Method::GET => match self.sync_current_link(document_id).await {
                Ok(link) => write_json(200, &json!({"link": link})),
                Err(response) => response,
            },
            Method::DELETE => match self.store.catalog.delete_sync_link(document_id).await {
                Ok(_) => no_content(),
                Err(error) => sync_storage_failure("forget the link", &error),
            },
            Method::PUT => {
                self.put_sync_link(request, arrival, slug, who.principal_key(), document_id)
                    .await
            }
            _ => plain(405, "method not allowed"),
        }
    }

    async fn put_sync_link(
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
        let who = match self.sync_editor(slug, &headers, arrival).await {
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
        if !is_remote_kind(&input.remote_kind) {
            return invalid("remote_kind must be network or local");
        }
        if let Err(why) = check_remote_label(&input.remote_label) {
            return invalid(&why);
        }
        if !is_branch(&input.branch) {
            return invalid("branch must be 1 to 255 characters without control characters");
        }
        if !(input.folder.is_empty() || is_relative_path(&input.folder)) {
            return invalid("folder must be empty or a relative path without dot segments");
        }
        if input
            .checkpoint
            .as_deref()
            .is_some_and(|checkpoint| !is_hex(checkpoint, 40))
        {
            return invalid("checkpoint must be null or 40 lowercase hex digits");
        }
        if let Err(why) = check_paths(&input.synced_paths, max_files) {
            return invalid(&why);
        }
        if let Some(revision) = input.revision {
            if revision < 1 {
                return invalid("revision must be null or a stored revision");
            }
        }
        let write = SyncLinkWrite {
            remote_kind: input.remote_kind,
            remote_label: input.remote_label,
            connection_id: input.connection_id,
            branch: input.branch,
            folder: input.folder,
            checkpoint: input.checkpoint,
            synced_paths: input.synced_paths,
            connected_by: who.account_id(),
            pulled_by: None,
            pushed_by: if input.pushed { who.account_id() } else { None },
        };
        match self
            .store
            .catalog
            .put_sync_link_now(document_id, input.revision, &write)
            .await
        {
            Ok(Some(link)) => write_json(200, &json!({"link": self.sync_link_json(&link).await})),
            Ok(None) => {
                let current = match self.sync_current_link(document_id).await {
                    Ok(current) => current,
                    Err(response) => return response,
                };
                write_json(409, &json!({"error": "the link changed", "link": current}))
            }
            Err(error) => sync_storage_failure("save the link", &error),
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
        let (first, document_id) = match self.sync_editor(slug, &headers, arrival).await {
            Ok(found) => found,
            Err(response) => return response,
        };
        // The deployment's log quota bounds a pull's text and its synced paths
        // together, the same ceiling a socket update has.
        let body = match to_bytes(request.into_body(), self.config.log_quota_bytes).await {
            Ok(body) => body,
            Err(_) => return write_json(413, &json!({"error": "the change is too large"})),
        };
        let principal = first.principal_key();
        let who = match self.sync_editor(slug, &headers, arrival).await {
            Ok((who, _)) if who.principal_key() == principal => who,
            Ok(_) => return no_such_document(),
            Err(response) => return response,
        };
        let input: SourceChangeInput = match serde_json::from_slice(&body) {
            Ok(input) => input,
            Err(_) => return write_json(400, &json!({"error": "the change is malformed"})),
        };
        let invalid = |why: &str| write_json(400, &json!({"error": why}));
        if !is_hex(&input.sync.checkpoint, 40) {
            return invalid("sync.checkpoint must be 40 lowercase hex digits");
        }
        if let Err(why) = check_paths(&input.sync.synced_paths, self.config.max_files) {
            return invalid(&why);
        }
        let row = match self.store.catalog.sync_link(document_id).await {
            Ok(Some(row)) => row,
            Ok(None) => return stale_reply(None),
            Err(error) => return sync_storage_failure("read the link", &error),
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
                epoch: "sync".into(),
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
            operation_scope: format!("sync:{document_id}:{principal}"),
        };
        let write = SyncLinkWrite {
            remote_kind: row.remote_kind.clone(),
            remote_label: row.remote_label.clone(),
            connection_id: row.connection_id,
            branch: row.branch.clone(),
            folder: row.folder.clone(),
            checkpoint: Some(input.sync.checkpoint),
            synced_paths: input.sync.synced_paths,
            connected_by: None,
            pulled_by: who.account_id(),
            pushed_by: None,
        };
        let revision = input.sync.revision;
        let hook: TransactHook = Box::new(move |connection| {
            Box::pin(async move {
                match PostgresCatalog::put_sync_link(
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
            match self.sync_editor(slug, &headers, arrival).await {
                Ok((now, _)) if now.principal_key() == principal => Ok(()),
                _ => Err(AgentError::Conflict("document access changed".into())),
            }
        };
        let applied = room
            .apply_source_change(
                change,
                Origin::Sync {
                    provider: "git".into(),
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
        let link = match self.sync_current_link(document_id).await {
            Ok(link) => link,
            Err(response) => return response,
        };
        write_json(200, &json!({"receipt": receipt, "link": link}))
    }

    async fn source_change_refusal(&self, document_id: uuid::Uuid, error: AgentError) -> Reply {
        match error {
            AgentError::Hook(why) if why == "stale" => {
                match self.sync_current_link(document_id).await {
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
}

/// A storage failure answers a retryable 503 and logs what failed, never
/// anything the caller sent.
fn sync_storage_failure(what: &str, error: &dyn std::fmt::Display) -> Reply {
    tracing::warn!("could not {what}: {error}");
    write_json(
        503,
        &json!({"error": format!("could not {what}"), "retryable": true}),
    )
}

#[cfg(test)]
#[path = "sync_link_tests.rs"]
mod tests;
