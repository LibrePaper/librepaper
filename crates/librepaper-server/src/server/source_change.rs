//! The source-change route: one checked change to a project, pushed from the
//! command line or from a CI job.
//!
//! The caller plans the change against a pull of the project. The server
//! checks it whole against the text a person sees and applies all of it or
//! none of it. The version it writes is named by the caller's message.

use super::*;
use librepaper_room::room::agent::OperationKey;
use librepaper_room::room::source_change::{
    AgentError, Consistency, CreateFile, DeleteFile, Dependency, Origin, Patch, PutBinary,
    SourceChange,
};

/// The longest a push message may be, in characters.
const SOURCE_MESSAGE_MAX_CHARS: usize = 200;
/// The longest an author name may be, in characters.
const MAX_AUTHOR: usize = 100;

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

/// Who pushes and what the version is called. `author` is a name the caller
/// supplies, as a git author is; the account behind the credential is
/// recorded beside it.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SyncInput {
    provider: String,
    message: String,
    author: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SourceChangeInput {
    operation: String,
    change: ChangeInput,
    sync: SyncInput,
}

/// Whether a name is between one and `max` characters.
fn within(text: &str, max: usize) -> bool {
    (1..=max).contains(&text.chars().count())
}

impl Server {
    /// The document and its editor, or the reply that refuses.
    #[allow(clippy::result_large_err)]
    async fn change_editor(
        &self,
        slug: &str,
        headers: &HeaderMap,
        arrival: &Arrival,
    ) -> Result<Viewer, Reply> {
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
        Ok(who)
    }

    /// `POST /api/documents/{slug}/source-change`: one checked change to the
    /// project, applied whole or not at all.
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
        let first = match self.change_editor(slug, &headers, arrival).await {
            Ok(found) => found,
            Err(response) => return response,
        };
        // The deployment's log quota bounds a push's text, the same ceiling a
        // socket update has.
        let body = match to_bytes(request.into_body(), self.config.log_quota_bytes).await {
            Ok(body) => body,
            Err(_) => return write_json(413, &json!({"error": "the change is too large"})),
        };
        // Access may have been withdrawn while the body was arriving.
        let principal = first.principal_key();
        let who = match self.change_editor(slug, &headers, arrival).await {
            Ok(who) if who.principal_key() == principal => who,
            Ok(_) => return no_such_document(),
            Err(response) => return response,
        };
        let input: SourceChangeInput = match serde_json::from_slice(&body) {
            Ok(input) => input,
            Err(_) => return write_json(400, &json!({"error": "the change is malformed"})),
        };
        let invalid = |why: &str| write_json(400, &json!({"error": why}));
        if input.sync.provider != "cli" {
            return invalid("sync.provider must be cli");
        }
        if !within(&input.sync.message, SOURCE_MESSAGE_MAX_CHARS) {
            return invalid("sync.message must be 1 to 200 characters");
        }
        if input
            .sync
            .author
            .as_deref()
            .is_some_and(|author| !within(author, MAX_AUTHOR))
        {
            return invalid("sync.author must be 1 to 100 characters");
        }
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
            request_digest: librepaper_base::digest::Digest::of(&body).to_hex(),
            message: Some(input.sync.message),
            author: input.sync.author,
        };
        let ceiling = self.ceiling_for(&who.id);
        let mut authority =
            match who.agent_authority(ceiling, format!("sync:{}:{principal}", room.document_id)) {
                Ok(authority) => authority,
                Err(InvalidCredential) => {
                    return change_refusal(AgentError::Conflict("document access changed".into()))
                }
            };
        // A token has no key of its own, so a push with no author is the
        // account's.
        if authority.author_key.is_empty() {
            authority.author_key = who.attribution().display().to_owned();
        }
        let recheck = || async {
            match self.change_editor(slug, &headers, arrival).await {
                Ok(now) if now.principal_key() == principal => Ok(()),
                _ => Err(AgentError::Conflict("document access changed".into())),
            }
        };
        let applied = room
            .apply_source_change(
                change,
                Origin::Sync {
                    provider: "cli".into(),
                },
                authority,
                None,
                recheck,
            )
            .await;
        match applied {
            Ok(receipt) => write_json(200, &json!({"label": receipt.label, "receipt": receipt})),
            Err(error) => change_refusal(error),
        }
    }
}

/// The reply a refused change gets. A changed file names every path that
/// moved, so the caller can pull once and plan again.
fn change_refusal(error: AgentError) -> Reply {
    match error {
        AgentError::Changed(paths) => write_json(
            409,
            &json!({
                "error": "files changed since the change was planned",
                "reason": "changed",
                "paths": paths,
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
        AgentError::Conflict(why) => write_json(409, &json!({"error": why, "reason": "conflict"})),
        AgentError::Storage(why) => {
            tracing::warn!("source change failed in storage: {why}");
            write_json(
                503,
                &json!({"error": "could not apply the change", "retryable": true}),
            )
        }
    }
}

#[cfg(test)]
#[path = "source_change_tests.rs"]
mod tests;
