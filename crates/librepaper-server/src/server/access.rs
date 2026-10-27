//! The access guard: one place that decides whether a caller may touch a
//! document, and the only way a handler reaches a mutation.
//!
//! [`Server::authorize`] runs the whole decision once: the document exists,
//! the credential resolved, a shared link comes with a sign-in, the caller
//! may read, and the caller holds the rung the route needs. For an MCP call
//! it also pins the actor scope and the runner lease. The answer is an
//! [`Authorized`], which keeps the [`Caller`] so the same decision can be
//! rerun later.
//!
//! An `Authorized` is a past answer, and nothing in its type says it is still
//! true. Freshness is enforced where the write happens. [`Server::mutate`]
//! reruns the full decision against the current catalogue and credentials and
//! then runs the write with the fresh [`Viewer`], so a room or store call
//! takes the viewer `mutate` handed it and never one the handler kept.
//! [`Server::recheck`] is the same decision for a gate that guards a write
//! owned by somebody else, such as the fence a source change runs just before
//! it commits.

use std::future::Future;
use std::marker::PhantomData;

use super::*;
use librepaper_room::room::refusal::Kind;

/// One rung of the access ladder, as a type, so a handler that needs the
/// editor rung cannot be handed an answer that only checked reading.
pub(super) trait Rung: Send + Sync + 'static {
    /// The role a caller must hold.
    const ROLE: Role;

    /// What a caller is told when this rung was withdrawn after it was
    /// admitted.
    fn lost() -> Refusal;
}

/// The rungs a route can ask for.
pub(super) mod rung {
    use super::*;

    pub(in crate::server) struct Read;
    pub(in crate::server) struct Comment;
    pub(in crate::server) struct Edit;
    pub(in crate::server) struct Owner;

    impl Rung for Read {
        const ROLE: Role = Role::Reader;
        fn lost() -> Refusal {
            Refusal::not_written(Kind::Forbidden, "document access changed")
        }
    }

    impl Rung for Comment {
        const ROLE: Role = Role::Commenter;
        fn lost() -> Refusal {
            Refusal::not_written(Kind::Forbidden, "comment access changed")
        }
    }

    impl Rung for Edit {
        const ROLE: Role = Role::Editor;
        fn lost() -> Refusal {
            Refusal::not_written(Kind::Forbidden, "edit access changed")
        }
    }

    impl Rung for Owner {
        const ROLE: Role = Role::Owner;
        fn lost() -> Refusal {
            Refusal::not_written(Kind::Forbidden, "ownership changed")
        }
    }
}

/// The runner lease an MCP call arrived with: the conversation the runner
/// speaks for and the execution epoch it was issued.
#[derive(Clone, Debug)]
pub(super) struct RunnerLease {
    conversation: String,
    epoch: String,
}

impl RunnerLease {
    /// The lease the request headers name, if they name one. Headers that
    /// name half a lease, or a lease that is not well formed, are refused.
    fn of(headers: &HeaderMap) -> Result<Option<Self>, Refusal> {
        let refuse = |message: &'static str| Refusal::not_written(Kind::Forbidden, message);
        if headers.contains_key("x-librepaper-execution-epoch")
            && (mcp::runner_execution_epoch(headers).is_empty()
                || !headers.contains_key("x-librepaper-runner-conversation"))
        {
            return Err(refuse("runner epoch requires valid runner provenance"));
        }
        let Some(conversation) = headers.get("x-librepaper-runner-conversation") else {
            return Ok(None);
        };
        let conversation = conversation
            .to_str()
            .map_err(|_| refuse("runner conversation is invalid"))?;
        if conversation.is_empty() {
            return Err(refuse("runner conversation is missing"));
        }
        let epoch = mcp::runner_execution_epoch(headers);
        if epoch.is_empty() {
            return Err(refuse("runner execution lease is missing"));
        }
        Ok(Some(Self {
            conversation: conversation.to_owned(),
            epoch,
        }))
    }
}

/// Everything a recheck needs to rerun the decision that admitted a request:
/// the credentials it carried, how it arrived, and for an MCP call the actor
/// scope it was admitted as and the runner lease it holds.
#[derive(Clone, Debug)]
pub(super) struct Caller {
    headers: HeaderMap,
    arrival: Arrival,
    query: Option<String>,
    /// The actor scope an MCP call was admitted as. A recheck that finds a
    /// different scope refuses, since the call now belongs to somebody else.
    actor: Option<String>,
    runner: Option<RunnerLease>,
}

impl Caller {
    /// The actor scope the call was admitted as, when it came over MCP.
    pub(super) fn actor(&self) -> Option<&str> {
        self.actor.as_deref()
    }
}

/// A past answer to "may this caller do this here". See the module comment
/// for why holding one proves nothing about now.
pub(super) struct Authorized<R: Rung> {
    pub(super) slug: String,
    pub(super) entry: IndexEntry,
    pub(super) viewer: Viewer,
    pub(super) caller: Caller,
    _rung: PhantomData<fn() -> R>,
}

impl<R: Rung> Authorized<R> {
    /// The same answer at a higher rung, for a handler that serves reading
    /// and writing from one route. The refusal is the higher rung's `lost`.
    pub(super) fn require<S: Rung>(self) -> Result<Authorized<S>, Refusal> {
        if !self.viewer.at_least(S::ROLE) {
            return Err(S::lost());
        }
        Ok(Authorized {
            slug: self.slug,
            entry: self.entry,
            viewer: self.viewer,
            caller: self.caller,
            _rung: PhantomData,
        })
    }
}

/// The HTTP answer for a refusal the guard produced. The body is the
/// `{"error": ...}` shape these routes have always used, with `retryable` on
/// a storage failure.
pub(super) fn deny(refusal: &Refusal) -> Reply {
    let mut body = json!({"error": refusal.message});
    if refusal.retryable() {
        body["retryable"] = json!(true);
    }
    write_json(refusal.status(), &body)
}

/// Whether a comment must carry the render digest of the page it was written
/// against. An editor annotates the live document, and everybody else
/// annotates the exact projection that produced their page.
pub(super) fn needs_render_digest(who: &Viewer, incoming: &RoomMessage) -> bool {
    incoming.kind() == "comment"
        && !who.at_least(Role::Editor)
        && incoming.render_digest().is_empty()
}

fn not_found() -> Refusal {
    Refusal::not_written(Kind::NotFound, "not found")
}

fn authentication_expired() -> Refusal {
    Refusal::not_written(Kind::Unauthenticated, "authentication expired or was revoked")
}

fn sign_in_to_read() -> Refusal {
    Refusal::not_written(Kind::Unauthenticated, "sign in to read this shared document")
}

impl Server {
    /// Runs the full decision once for a request that went through the
    /// admission middleware, so the caller's identity is the one the context
    /// already resolved. `query` is the link key a socket handshake carries.
    pub(super) async fn authorize<R: Rung>(
        &self,
        context: &RequestContext,
        headers: &HeaderMap,
        slug: &str,
        query: Option<&str>,
    ) -> Result<Authorized<R>, Refusal> {
        self.admit(Some(context.identity()), headers, &context.arrival, slug, query, false)
            .await
    }

    /// The same for a handler with no request context, which resolves the
    /// credential itself.
    pub(super) async fn authorize_fresh<R: Rung>(
        &self,
        headers: &HeaderMap,
        arrival: &Arrival,
        slug: &str,
        query: Option<&str>,
    ) -> Result<Authorized<R>, Refusal> {
        self.admit(None, headers, arrival, slug, query, false).await
    }

    /// The same for an MCP tool call, which also pins the actor scope and the
    /// runner lease. `headers` are the ones the tool call runs under.
    pub(super) async fn authorize_mcp<R: Rung>(
        &self,
        context: &RequestContext,
        headers: &HeaderMap,
        slug: &str,
    ) -> Result<Authorized<R>, Refusal> {
        self.admit(Some(context.identity()), headers, &context.arrival, slug, None, true)
            .await
    }

    /// Reruns the full decision, then runs `write` with the fresh viewer and
    /// entry. The write starts the moment the decision ends, with no await
    /// the caller controls in between.
    pub(super) async fn mutate<R, T, F, Fut>(
        &self,
        auth: &Authorized<R>,
        write: F,
    ) -> Result<T, Refusal>
    where
        R: Rung,
        F: FnOnce(Viewer, IndexEntry) -> Fut,
        Fut: Future<Output = T>,
    {
        let (viewer, entry) = self.recheck(auth).await?;
        Ok(write(viewer, entry).await)
    }

    /// Reruns the full decision and hands back the fresh viewer and entry.
    /// For a gate in front of a write this guard does not run itself.
    pub(super) async fn recheck<R: Rung>(
        &self,
        auth: &Authorized<R>,
    ) -> Result<(Viewer, IndexEntry), Refusal> {
        let caller = &auth.caller;
        let entry = self.entry_of(&auth.slug).await?;
        let viewer = self
            .viewer(&entry, &caller.headers, &caller.arrival, caller.query.as_deref())
            .await;
        self.gate(&entry, &viewer, R::ROLE, R::lost)?;
        if let Some(expected) = &caller.actor {
            if &self.actor_of(&auth.slug, &viewer, &caller.headers, &caller.arrival) != expected {
                return Err(R::lost());
            }
        }
        if let Some(lease) = &caller.runner {
            if !self
                .chat
                .valid_agent_lease(&auth.slug, &lease.conversation, &lease.epoch)
                .await
            {
                return Err(Refusal::not_written(
                    Kind::Forbidden,
                    "runner execution lease has expired",
                ));
            }
        }
        Ok((viewer, entry))
    }

    async fn admit<R: Rung>(
        &self,
        identity: Option<Identity>,
        headers: &HeaderMap,
        arrival: &Arrival,
        slug: &str,
        query: Option<&str>,
        mcp: bool,
    ) -> Result<Authorized<R>, Refusal> {
        let entry = self.entry_of(slug).await?;
        let viewer = match identity {
            Some(identity) => self.viewer_as(&entry, identity, headers, arrival, query),
            None => self.viewer(&entry, headers, arrival, query).await,
        };
        self.gate(&entry, &viewer, R::ROLE, not_found)?;
        let (actor, runner) = if mcp {
            (
                Some(self.actor_of(slug, &viewer, headers, arrival)),
                RunnerLease::of(headers)?,
            )
        } else {
            (None, None)
        };
        let caller = Caller {
            headers: headers.clone(),
            arrival: arrival.clone(),
            query: query.map(str::to_owned),
            actor,
            runner,
        };
        if let Some(lease) = &caller.runner {
            if !self
                .chat
                .valid_agent_lease(slug, &lease.conversation, &lease.epoch)
                .await
            {
                return Err(Refusal::not_written(
                    Kind::Forbidden,
                    "runner execution lease has expired",
                ));
            }
        }
        Ok(Authorized {
            slug: slug.to_owned(),
            entry,
            viewer,
            caller,
            _rung: PhantomData,
        })
    }

    /// The document, or the refusal for a missing one or a failed lookup.
    async fn entry_of(&self, slug: &str) -> Result<IndexEntry, Refusal> {
        match self.store.get_result(slug).await {
            Ok(Some(entry)) => Ok(entry),
            Ok(None) => Err(not_found()),
            Err(error) => {
                tracing::warn!("could not look up {slug}: {error}");
                Err(Refusal::not_written(
                    Kind::Unavailable,
                    "document lookup failed",
                ))
            }
        }
    }

    /// The credential, sign-in, read and rung checks, in that order. A caller
    /// who may not read, or lacks the rung, gets `denied`.
    fn gate(
        &self,
        entry: &IndexEntry,
        who: &Viewer,
        rung: Role,
        denied: impl FnOnce() -> Refusal,
    ) -> Result<(), Refusal> {
        if who.auth_failed {
            return Err(authentication_expired());
        }
        if self.needs_sign_in(entry, who) {
            return Err(sign_in_to_read());
        }
        if !who.at_least(rung) || !self.may_read(entry, who) {
            return Err(denied());
        }
        Ok(())
    }

    fn actor_of(&self, slug: &str, who: &Viewer, headers: &HeaderMap, arrival: &Arrival) -> String {
        mcp::actor_scope(slug, who, &self.comment_author(headers, arrival, &who.id))
    }
}
