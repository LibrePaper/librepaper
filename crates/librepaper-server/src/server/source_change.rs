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
        if !self.valid_slug(slug) {
            return plain(400, "bad slug");
        }
        let auth = match self
            .authorize_fresh::<access::rung::Read>(&headers, arrival, slug, None)
            .await
        {
            Ok(auth) => auth,
            Err(refusal) => return access::deny(&refusal),
        };
        let Ok(auth) = auth.require::<access::rung::Edit>() else {
            return write_json(403, &json!({"error": "edit access required"}));
        };
        // The deployment's log quota bounds a push's text, the same ceiling a
        // socket update has.
        let body = match to_bytes(request.into_body(), self.config.log_quota_bytes).await {
            Ok(body) => body,
            Err(_) => return write_json(413, &json!({"error": "the change is too large"})),
        };
        // Access may have been withdrawn while the body was arriving, so the
        // change is applied through `mutate` below.
        let principal = auth.viewer.principal_key();
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
        let principal = &principal;
        let fence = &auth;
        let recheck = || async {
            match self.recheck(fence).await {
                Ok((now, _)) if now.principal_key() == *principal => Ok(()),
                _ => Err(AgentError::Conflict("document access changed".into())),
            }
        };
        let applied = self
            .mutate(&auth, |who, _| {
                async move {
                    if who.principal_key() != *principal {
                        return Err(AgentError::Conflict("document access changed".into()));
                    }
                    let ceiling = self.ceiling_for(&who.id);
                    let mut authority = match who
                        .agent_authority(ceiling, format!("sync:{}:{principal}", room.document_id))
                    {
                        Ok(authority) => authority,
                        Err(InvalidCredential) => {
                            return Err(AgentError::Conflict("document access changed".into()))
                        }
                    };
                    // A token has no key of its own, so a push with no author is the
                    // account's.
                    if authority.author_key.is_empty() {
                        authority.author_key = who.attribution().display().to_owned();
                    }
                    room.apply_source_change(
                        change,
                        Origin::Sync {
                            provider: "cli".into(),
                        },
                        authority,
                        None,
                        recheck,
                    )
                    .await
                }
            })
            .await;
        match applied {
            Ok(Ok(receipt)) => {
                write_json(200, &json!({"label": receipt.label, "receipt": receipt}))
            }
            Ok(Err(error)) => change_refusal(error),
            Err(refusal) => access::deny(&refusal),
        }
    }
}

/// The reply a refused change gets. A changed file names every path that
/// moved, so the caller can pull once and plan again.
fn change_refusal(error: AgentError) -> Reply {
    if let AgentError::Storage(why) = &error {
        tracing::warn!("source change failed in storage: {why}");
    }
    let fields = match &error {
        AgentError::Changed(paths) => vec![("paths", json!(paths))],
        AgentError::Suggestion { path, change } => {
            vec![("path", json!(path)), ("change", json!(change))]
        }
        _ => Vec::new(),
    };
    refusal::http_with(&Refusal::from(&error), &fields)
}

#[cfg(test)]
#[path = "source_change_tests.rs"]
mod tests;
