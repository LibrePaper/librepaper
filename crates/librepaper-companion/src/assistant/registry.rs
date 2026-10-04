//! The companion's session registry.
//!
//! `local start` used to hand each (document, conversation) pair to a
//! spawned `librepaper run-agent` process; it now runs the same run loop
//! (`runtime::run`) as a supervised tokio task inside its own process. This
//! registry is what a runner's flocked `runner.lock` used to be: the sole
//! proof that a session is live. A key present in `sessions` with a
//! `Running` entry is running; nothing else needs to be asked.
//!
//! Locking discipline: the map is a plain `std::sync::Mutex`, which cannot
//! be held across an `.await`. Every method here takes the lock only long
//! enough to look up, insert or remove an entry, never while starting,
//! stopping or polling the task itself.

use super::lifecycle::{self, RunnerState, StatusHandle, StopSignal};
use super::runtime;
use crate::automation::peer::{AutomationPeer, DocumentLink};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex;

type SharedAgentGrant = std::sync::Arc<std::sync::RwLock<String>>;
type OwnedAgentGrant = (Option<String>, SharedAgentGrant);

enum Entry {
    /// A session task is spawned and, as far as the registry knows, still
    /// running. `is_finished()` on the handle can already be true; nothing
    /// reaps it until the next call touches this key.
    Running {
        config_hash: String,
        origin: Option<String>,
        status: StatusHandle,
        stop: StopSignal,
        handle: tokio::task::JoinHandle<Result<(), String>>,
        /// Rotated by the paired browser while its session remains active.
        grant: Option<SharedAgentGrant>,
    },
    /// The task has exited (normally, with an error, or by panicking) and no
    /// task handle is left to hold. Kept only so one subsequent status poll
    /// can see why, exactly as `runner.status.json` used to survive its
    /// runner's exit.
    Terminal(lifecycle::StatusSnapshot),
}

pub(crate) struct SessionRegistry {
    sessions: Mutex<HashMap<String, Entry>>,
    /// Serialises the check-then-spawn in `start`. Without it two sidebar
    /// requests for one conversation could both find nothing running and
    /// both spawn, leaving two assistants on one conversation. Held across
    /// the stop and the spawn, never across the wait for readiness.
    start_gate: tokio::sync::Mutex<()>,
    /// The companion's own state directory, i.e. the same one `Inner` was
    /// built with. Threaded down into `runtime::Config` so a session's
    /// lookups and its private connection record agree on where the
    /// companion actually keeps them, regardless of this process's own
    /// XDG_STATE_HOME or HOME.
    state_home: PathBuf,
}

/// A session directory older than this many is ignored during startup
/// recovery, so a companion that has accumulated years of one-off
/// conversations does not turn every restart into an unbounded directory
/// walk. Ordered oldest-first by directory name, which is a hex digest and
/// carries no chronological meaning of its own -- the bound exists to cap
/// the walk, not to prefer any particular session.
const MAX_RECOVERED_SESSIONS: usize = 500;

fn ensure_origin_owner(owner: Option<&str>, requester: &str) -> Result<(), String> {
    let Some(owner) = owner else {
        return Err(
            "the running assistant session has no paired-site owner; stop and restart it".into(),
        );
    };
    if crate::local::pairing::normalize_origin(owner)
        == crate::local::pairing::normalize_origin(requester)
    {
        Ok(())
    } else {
        Err("the assistant session is already owned by another paired site".into())
    }
}

fn replace_agent_grant(
    owner: Option<&str>,
    requester: &str,
    grant: &std::sync::RwLock<String>,
    token: &str,
) -> Result<(), String> {
    ensure_origin_owner(owner, requester)?;
    *grant.write().unwrap_or_else(|error| error.into_inner()) = token.to_string();
    Ok(())
}

impl SessionRegistry {
    pub(crate) fn new(state_home: PathBuf) -> Self {
        Self {
            sessions: Mutex::new(HashMap::new()),
            start_gate: tokio::sync::Mutex::new(()),
            state_home,
        }
    }

    /// Drop a finished task's handle, replacing it with the terminal status
    /// it ended on. A no-op when the entry is still running or already
    /// terminal. Never awaits anything that can block: a finished
    /// `JoinHandle` resolves immediately.
    async fn reap(&self, key: &str) {
        let running = {
            let mut sessions = self.sessions.lock().unwrap_or_else(|e| e.into_inner());
            let finished = matches!(
                sessions.get(key),
                Some(Entry::Running { handle, .. }) if handle.is_finished()
            );
            if finished {
                sessions.remove(key)
            } else {
                None
            }
        };
        let Some(Entry::Running { handle, .. }) = running else {
            return;
        };
        let snapshot = match handle.await {
            Ok(Ok(())) => lifecycle::StatusSnapshot {
                state: Some(RunnerState::Stopped),
                task_id: None,
                detail: None,
            },
            Ok(Err(detail)) => lifecycle::StatusSnapshot {
                state: Some(RunnerState::Failed),
                task_id: None,
                detail: Some(detail),
            },
            Err(join_error) => lifecycle::StatusSnapshot {
                state: Some(RunnerState::Failed),
                task_id: None,
                detail: Some(format!(
                    "the assistant session ended unexpectedly: {join_error}"
                )),
            },
        };
        let mut sessions = self.sessions.lock().unwrap_or_else(|e| e.into_inner());
        sessions.insert(key.to_string(), Entry::Terminal(snapshot));
    }

    /// Idempotently ensure a session with this configuration is running for
    /// (link, conversation), then wait for it to settle into a state the
    /// sidebar can act on. A session already running with a matching
    /// `config_hash` is left alone; a different one is hard-stopped first.
    /// Bounded the same 75 seconds the old background-process poll was, so a
    /// wedged agent handshake still surfaces as a timeout rather than hanging
    /// the browser's request forever.
    #[allow(clippy::too_many_arguments)] // Keep the authenticated owner explicit alongside the session credentials.
    pub(crate) async fn start(
        &self,
        pairing: &crate::local::pairing::PairingStore,
        link: &DocumentLink,
        origin: &str,
        pairing_token: &str,
        conversation: &str,
        chat_token: &str,
        agent_token: &str,
        agent: &[String],
        agent_environment: &[(String, String)],
    ) -> Result<(), String> {
        let key = lifecycle::session_key(link, conversation)?;
        let gate = self.start_gate.lock().await;
        let requester = crate::local::pairing::normalize_origin(origin);
        if !pairing.authenticate(origin, pairing_token) {
            return Err("the local pairing is no longer valid; reconnect this site".into());
        }
        self.reap(&key).await;
        let wanted = lifecycle::configuration_hash(link, agent);
        let current = {
            let sessions = self.sessions.lock().unwrap_or_else(|e| e.into_inner());
            match sessions.get(&key) {
                Some(Entry::Running {
                    config_hash,
                    origin,
                    ..
                }) => Some((config_hash.clone(), origin.clone())),
                _ => None,
            }
        };
        if let Some((_, owner)) = &current {
            ensure_origin_owner(owner.as_deref(), &requester)?;
        }
        let current_hash = current
            .as_ref()
            .map(|(config_hash, _)| config_hash.as_str());
        if current_hash != Some(wanted.as_str()) {
            if current_hash.is_some() {
                self.stop(link, conversation)
                    .await
                    .map_err(|error| error.to_string())?;
            }
            let peer = AutomationPeer::open_scoped(link.clone(), agent_token.to_string()).await?;
            let config = runtime::config(
                conversation.to_string(),
                Some(chat_token.to_string()),
                agent.to_vec(),
                self.state_home.clone(),
                agent_environment.to_vec(),
            )?;
            let admission = pairing.admission_gate().lock().await;
            if !pairing.authenticate(origin, pairing_token) {
                return Err("the local pairing is no longer valid; reconnect this site".into());
            }
            self.publish_runner_connection(link, origin, conversation, chat_token)?;
            let grant = Some(peer.token_source());
            self.spawn_running(
                &key,
                &wanted,
                Some(requester),
                grant,
                move |status, stop| async move { runtime::run(&peer, config, status, stop).await },
            );
            drop(admission);
        } else {
            self.validate_agent_token(link, agent_token).await?;
            let (owner, grant) = self.active_grant(&key)?;
            let admission = pairing.admission_gate().lock().await;
            if !pairing.authenticate(origin, pairing_token) {
                return Err("the local pairing is no longer valid; reconnect this site".into());
            }
            self.publish_runner_connection(link, origin, conversation, chat_token)?;
            replace_agent_grant(owner.as_deref(), &requester, &grant, agent_token)?;
            drop(admission);
        }
        drop(gate);
        self.wait_until_settled(&key).await
    }

    /// Spawn `body` as the session task for `key` and register it as
    /// `Running`, replacing whatever was there. The only thing production
    /// code passes here is `runtime::run` bound to a real `AutomationPeer`;
    /// registry tests pass an arbitrary async body instead, so the
    /// registry's own mechanics -- idempotent replace, two sessions running
    /// side by side, a hard stop, a panicking task -- are exercised without
    /// an ACP agent or network access.
    fn spawn_running<F, Fut>(
        &self,
        key: &str,
        config_hash: &str,
        origin: Option<String>,
        grant: Option<SharedAgentGrant>,
        body: F,
    ) where
        F: FnOnce(StatusHandle, StopSignal) -> Fut,
        Fut: std::future::Future<Output = Result<(), String>> + Send + 'static,
    {
        let status = StatusHandle::new();
        let stop = StopSignal::new();
        let handle = tokio::spawn(body(status.clone(), stop.clone()));
        let mut sessions = self.sessions.lock().unwrap_or_else(|e| e.into_inner());
        // Dropping a JoinHandle detaches its task rather than ending it, so a
        // session this replaces is aborted, never orphaned.
        if let Some(Entry::Running {
            handle: replaced, ..
        }) = sessions.remove(key)
        {
            replaced.abort();
        }
        sessions.insert(
            key.to_string(),
            Entry::Running {
                config_hash: config_hash.to_string(),
                origin,
                status,
                stop,
                handle,
                grant,
            },
        );
    }

    /// Replace the active runner's scoped server credential after a paired
    /// browser has renewed it. The peer reads this shared value on each HTTP
    /// request and websocket reconnect.
    pub(crate) async fn renew_agent_token(
        &self,
        link: &DocumentLink,
        origin: &str,
        pairing_token: &str,
        conversation: &str,
        token: &str,
    ) -> Result<(), String> {
        let _gate = self.start_gate.lock().await;
        let pairing = crate::local::pairing::PairingStore::new(&self.state_home, None);
        if !pairing.authenticate(origin, pairing_token) {
            return Err("the local pairing is no longer valid; reconnect this site".into());
        }
        let requester = crate::local::pairing::normalize_origin(origin);
        let key = lifecycle::session_key(link, conversation)?;
        let (owner, _) = self.active_grant(&key)?;
        ensure_origin_owner(owner.as_deref(), &requester)?;
        self.validate_agent_token(link, token).await?;
        if !pairing.authenticate(origin, pairing_token) {
            return Err("the local pairing is no longer valid; reconnect this site".into());
        }
        let (owner, grant) = self.active_grant(&key)?;
        replace_agent_grant(owner.as_deref(), &requester, &grant, token)
    }

    fn active_grant(&self, key: &str) -> Result<OwnedAgentGrant, String> {
        let sessions = self
            .sessions
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let Some(Entry::Running {
            origin,
            grant: Some(grant),
            ..
        }) = sessions.get(key)
        else {
            return Err("the assistant is not running; start it again from the document".into());
        };
        Ok((origin.clone(), grant.clone()))
    }

    fn publish_runner_connection(
        &self,
        link: &DocumentLink,
        origin: &str,
        conversation: &str,
        chat_token: &str,
    ) -> Result<(), String> {
        crate::local::connections::ConnectionStore::new(&self.state_home)
            .put_runner(&link.credential_url(), origin, conversation, chat_token)
            .map(|_| ())
    }

    async fn validate_agent_token(&self, link: &DocumentLink, token: &str) -> Result<(), String> {
        if !token.starts_with(librepaper_base::auth::AGENT_GRANT_PREFIX) {
            return Err("the renewed assistant authorization is invalid".into());
        }
        // Validate with the document server before replacing a working grant;
        // this rejects malformed, expired, revoked, wrong-document and wrong-
        // link credentials while preserving the active value on failure.
        AutomationPeer::open_scoped(link.clone(), token.to_string()).await?;
        Ok(())
    }

    async fn wait_until_settled(&self, key: &str) -> Result<(), String> {
        for _ in 0..750 {
            self.reap(key).await;
            let snapshot = {
                let sessions = self.sessions.lock().unwrap_or_else(|e| e.into_inner());
                match sessions.get(key) {
                    Some(Entry::Running { status, .. }) => status.get(),
                    Some(Entry::Terminal(snapshot)) => snapshot.clone(),
                    None => return Err("assistant session disappeared while starting".into()),
                }
            };
            match snapshot.state {
                Some(RunnerState::Ready | RunnerState::Working | RunnerState::NeedsInput) => {
                    return Ok(())
                }
                Some(RunnerState::Failed | RunnerState::Stopped) => {
                    return Err(snapshot
                        .detail
                        .unwrap_or_else(|| "the assistant session stopped".into()))
                }
                _ => {}
            }
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
        Err("the assistant did not become ready within 75 seconds".into())
    }

    /// The session's coarse status, or `None` when nothing by this key has
    /// ever run (never started, or the companion restarted since).
    pub(crate) async fn status(
        &self,
        link: &DocumentLink,
        conversation: &str,
    ) -> Result<lifecycle::StatusSnapshot, lifecycle::Error> {
        let key = lifecycle::session_key(link, conversation)
            .map_err(lifecycle::Error::InvalidConfiguration)?;
        self.status_by_key(&key)
            .await
            .ok_or(lifecycle::Error::NotRunning)
    }

    /// The local companion panel may inspect and stop sessions without a
    /// document origin. Session keys are opaque SHA-256 identifiers and the
    /// snapshot contains no document credential.
    pub(crate) async fn dashboard_sessions(&self) -> Vec<(String, lifecycle::StatusSnapshot)> {
        let ids: Vec<_> = self
            .sessions
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .keys()
            .cloned()
            .collect();
        for id in ids {
            self.reap(&id).await;
        }
        let sessions = self
            .sessions
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let mut rows: Vec<_> = sessions
            .iter()
            .map(|(id, entry)| {
                let status = match entry {
                    Entry::Running { status, .. } => status.get(),
                    Entry::Terminal(status) => status.clone(),
                };
                (id.clone(), status)
            })
            .collect();
        rows.sort_by(|left, right| left.0.cmp(&right.0));
        rows
    }

    pub(crate) async fn stop_dashboard_origin(&self, origin: &str) {
        let _gate = self.start_gate.lock().await;
        let keys: Vec<_> = {
            let sessions = self
                .sessions
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            sessions
                .iter()
                .filter_map(|(id, entry)| match entry {
                    Entry::Running {
                        origin: Some(session_origin),
                        ..
                    } if crate::local::pairing::normalize_origin(session_origin)
                        == crate::local::pairing::normalize_origin(origin) =>
                    {
                        Some(id.clone())
                    }
                    _ => None,
                })
                .collect()
        };
        for id in keys {
            let _ = self.stop_by_key(&id).await;
        }
    }

    pub(crate) async fn stop_dashboard_session(&self, id: &str) -> Result<(), String> {
        if id.len() != 64 || !id.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err("unknown assistant session".into());
        }
        self.stop_by_key(id)
            .await
            .map_err(|error| error.to_string())
    }

    async fn status_by_key(&self, key: &str) -> Option<lifecycle::StatusSnapshot> {
        self.reap(key).await;
        let sessions = self.sessions.lock().unwrap_or_else(|e| e.into_inner());
        match sessions.get(key) {
            Some(Entry::Running { status, .. }) => Some(status.get()),
            Some(Entry::Terminal(snapshot)) => Some(snapshot.clone()),
            None => None,
        }
    }

    /// Hard-stop a running session: request the cooperative shutdown a
    /// healthy session's own tick loop would take anyway, then abort its
    /// tokio task and await the abort. Aborting drops the task's whole stack,
    /// including its `acp::Agent`; per `agent-client-protocol` 2.2.0
    /// (`ChildGuard`), dropping the agent's connection future SIGKILLs the
    /// agent child's entire process group. That drop is what actually kills
    /// a wedged agent -- there is no separate PID or kill handle to reach for
    /// beyond aborting this task and waiting for the abort to land, which is
    /// why the await below is not optional. Once the task is gone, any task
    /// it left `working` is reconciled to `interrupted` from disk, the same
    /// semantics `recover_state` always had.
    pub(crate) async fn stop(
        &self,
        link: &DocumentLink,
        conversation: &str,
    ) -> Result<(), lifecycle::Error> {
        let key = lifecycle::session_key(link, conversation)
            .map_err(lifecycle::Error::InvalidConfiguration)?;
        self.stop_by_key(&key).await?;
        Ok(())
    }

    async fn stop_by_key(&self, key: &str) -> Result<(), lifecycle::Error> {
        self.reap(key).await;
        let entry = {
            let mut sessions = self.sessions.lock().unwrap_or_else(|e| e.into_inner());
            sessions.remove(key)
        };
        let Some(Entry::Running { stop, handle, .. }) = entry else {
            return Err(lifecycle::Error::NotRunning);
        };
        stop.request();
        handle.abort();
        let _ = handle.await;
        let state_path = self
            .state_home
            .join("librepaper")
            .join("assistant")
            .join(key)
            .join("runner.json");
        let _ = runtime::recover_state(&state_path);
        Ok(())
    }

    /// Reconcile every session directory left non-terminal by a previous
    /// companion process. Run once, at companion startup, before serving any
    /// assistant route. No session task is started here: recovery only marks
    /// interrupted work as such on disk, exactly as `recover_state` already
    /// did every time a runner process started or stopped; the sidebar
    /// starts a fresh session the normal way if the user asks for one.
    ///
    /// Bounded to [`MAX_RECOVERED_SESSIONS`] directories so a companion that
    /// has accumulated a large `assistant/` directory over time does not turn
    /// every restart into an unbounded walk; directories beyond the bound are
    /// left untouched; a task like the browser was clean up on its own is
    /// harmless. A restart should be rare compared to how often a runner used
    /// to restart, which is what makes this walk affordable at all.
    pub(crate) fn recover_at_startup(state_home: &std::path::Path) {
        let root = state_home.join("librepaper").join("assistant");
        let mut entries: Vec<_> = match std::fs::read_dir(&root) {
            Ok(entries) => entries.flatten().collect(),
            Err(_) => return,
        };
        entries.sort_by_key(|entry| entry.file_name());
        for entry in entries.into_iter().take(MAX_RECOVERED_SESSIONS) {
            let state_path = entry.path().join("runner.json");
            if state_path.is_file() {
                let _ = runtime::recover_state(&state_path);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::assistant::protocol::TaskStatus;
    use crate::assistant::task::{State, Task};
    use serde_json::json;

    #[test]
    fn running_session_cannot_be_reused_by_a_different_paired_origin() {
        assert!(
            ensure_origin_owner(Some("HTTPS://Papers.Example/"), "https://papers.example").is_ok()
        );
        assert_eq!(
            ensure_origin_owner(Some("https://papers.example"), "https://other.example")
                .unwrap_err(),
            "the assistant session is already owned by another paired site"
        );
    }

    #[test]
    fn cross_origin_renewal_cannot_replace_the_running_agent_grant() {
        let grant = std::sync::RwLock::new("original-grant".to_string());
        assert!(replace_agent_grant(
            Some("https://papers.example"),
            "https://other.example",
            &grant,
            "attacker-grant"
        )
        .is_err());
        assert_eq!(*grant.read().unwrap(), "original-grant");

        replace_agent_grant(
            Some("HTTPS://Papers.Example/"),
            "https://papers.example",
            &grant,
            "renewed-grant",
        )
        .unwrap();
        assert_eq!(*grant.read().unwrap(), "renewed-grant");
    }

    /// Two sessions started side by side are independent tasks: hard-stopping
    /// one must not disturb the other, which is the whole point of one
    /// supervised task per session rather than one shared loop.
    #[tokio::test]
    async fn stopping_one_session_leaves_a_concurrent_one_running() {
        let registry = SessionRegistry::new(PathBuf::from("/tmp/registry-test"));
        registry.spawn_running("one", "hash-1", None, None, |_status, _stop| async move {
            // Wedged mid-await, exactly like an agent handshake that never
            // returns: this task never checks `stop`, so only a hard abort
            // (never a cooperative signal) can end it.
            std::future::pending::<()>().await;
            Ok(())
        });
        registry.spawn_running("two", "hash-2", None, None, |status, stop| async move {
            status.set(RunnerState::Ready, None, None);
            while !stop.requested() {
                tokio::time::sleep(std::time::Duration::from_millis(5)).await;
            }
            Ok(())
        });
        registry.stop_by_key("one").await.unwrap();
        // A hard stop removes the entry outright; nothing is left to poll.
        assert!(registry.status_by_key("one").await.is_none());
        let two = registry.status_by_key("two").await.unwrap();
        assert_eq!(two.state, Some(RunnerState::Ready));
        registry.stop_by_key("two").await.unwrap();
    }

    /// Revoking a paired site's origin stops only sessions started by that
    /// authenticated requester, even when its document link belongs to a
    /// different server origin.
    #[tokio::test]
    async fn revoking_origin_stops_only_sessions_owned_by_that_origin() {
        let dir = tempfile::tempdir().unwrap();
        let registry = SessionRegistry::new(dir.path().to_path_buf());
        registry.spawn_running(
            "session-a",
            "hash-a",
            Some(crate::local::pairing::normalize_origin(
                "HTTPS://Papers.Example/",
            )),
            None,
            |_status, _stop| async move {
                std::future::pending::<()>().await;
                Ok(())
            },
        );
        registry.spawn_running(
            "session-b",
            "hash-b",
            Some("https://another.example".into()),
            None,
            |_status, _stop| async move {
                std::future::pending::<()>().await;
                Ok(())
            },
        );

        registry
            .stop_dashboard_origin("https://papers.example")
            .await;

        assert!(registry.status_by_key("session-a").await.is_none());
        assert!(registry.status_by_key("session-b").await.is_some());
        registry.stop_by_key("session-b").await.unwrap();
    }

    /// Connection publication belongs inside the authorized registry start:
    /// a rejected start from another paired site must not overwrite the
    /// current owner's durable link or chat credential.
    #[tokio::test]
    async fn rejected_cross_origin_start_preserves_runner_connection() {
        let dir = tempfile::tempdir().unwrap();
        let pairing = crate::local::pairing::PairingStore::new(dir.path(), None);
        let (_owner_token, _) = pairing.issue("https://owner.example", "Owner").unwrap();
        let (other_token, _) = pairing.issue("https://other.example", "Other").unwrap();
        let link = DocumentLink::parse("https://papers.example/docs/paper#k=secret", "")
            .unwrap();
        let conversation = "conversation-1";
        let connections = crate::local::connections::ConnectionStore::new(dir.path());
        let connection_name = connections
            .put_runner(
                &link.credential_url(),
                "https://owner.example",
                conversation,
                "owner-chat-token",
            )
            .unwrap();

        let registry = SessionRegistry::new(dir.path().to_path_buf());
        let key = lifecycle::session_key(&link, conversation).unwrap();
        registry.spawn_running(
            &key,
            "existing-config",
            Some("https://owner.example".into()),
            Some(std::sync::Arc::new(std::sync::RwLock::new(
                "owner-agent-token".into(),
            ))),
            |_status, _stop| async move {
                std::future::pending::<()>().await;
                Ok(())
            },
        );

        let error = registry
            .start(
                &pairing,
                &link,
                "https://other.example",
                &other_token,
                conversation,
                "other-chat-token",
                "invalid-agent-token",
                &[],
                &[],
            )
            .await
            .unwrap_err();
        assert_eq!(
            error,
            "the assistant session is already owned by another paired site"
        );
        let connection = connections.resolve(&connection_name).unwrap();
        assert_eq!(connection.origin, "https://owner.example");
        assert_eq!(connection.chat_token.as_deref(), Some("owner-chat-token"));
        let (_, owner_grant) = registry.active_grant(&key).unwrap();
        assert_eq!(*owner_grant.read().unwrap(), "owner-agent-token");
        registry.stop_by_key(&key).await.unwrap();
    }

    /// A session task that panics must surface as a failed status, not take
    /// the registry (or any other session) down with it -- `tokio::spawn`
    /// already turns the panic into a `JoinError` for the caller, so this is
    /// really a test that the registry actually reaps and reports it.
    #[tokio::test]
    async fn a_panicking_session_reports_failed_and_the_registry_keeps_serving() {
        let registry = SessionRegistry::new(PathBuf::from("/tmp/registry-test"));
        registry.spawn_running(
            "panics",
            "hash-1",
            None,
            None,
            |_status, _stop| async move { panic!("session task panicked") },
        );
        for _ in 0..100 {
            if let Some(snapshot) = registry.status_by_key("panics").await {
                if snapshot.state == Some(RunnerState::Failed) {
                    break;
                }
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        let snapshot = registry.status_by_key("panics").await.unwrap();
        assert_eq!(snapshot.state, Some(RunnerState::Failed));

        // The registry itself is unharmed: a second, healthy session still
        // starts and reports normally.
        registry.spawn_running(
            "healthy",
            "hash-1",
            None,
            None,
            |status, _stop| async move {
                status.set(RunnerState::Ready, None, None);
                std::future::pending::<()>().await;
                Ok(())
            },
        );
        let mut healthy = None;
        for _ in 0..100 {
            healthy = registry
                .status_by_key("healthy")
                .await
                .and_then(|s| s.state);
            if healthy == Some(RunnerState::Ready) {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        assert_eq!(healthy, Some(RunnerState::Ready));
        registry.stop_by_key("healthy").await.unwrap();
    }

    /// `State::load` alone already turns a `working` task into `interrupted`
    /// on any read; that is not what this exercises. What only startup
    /// recovery does is attach the journal's receipts to that task *before*
    /// anything else reads the directory, which is what lets a poll right
    /// after a companion restart see the effects a killed session already
    /// committed.
    #[test]
    fn startup_recovery_attaches_journal_receipts_left_by_a_killed_session() {
        let dir = tempfile::tempdir().unwrap();
        let session_dir = dir.path().join("librepaper").join("assistant").join("s1");
        std::fs::create_dir_all(&session_dir).unwrap();
        let mut state = State::default();
        let mut task =
            Task::from_message(&json!({"id":"one","role":"user","text":"Edit"})).unwrap();
        task.status = TaskStatus::Working;
        state.admit(task).unwrap();
        state.save(&session_dir.join("runner.json")).unwrap();
        let journal_path = session_dir.join("runner.journal.json");
        let request = json!({"operation":{"epoch":"e","id":"suggest"}});
        crate::assistant::journal::record_tool_call(
            &journal_path,
            "one",
            "document_propose",
            &request,
        )
        .unwrap();
        crate::assistant::journal::record_tool_result(
            &journal_path,
            "one",
            "document_propose",
            &request,
            &json!({"result":{"status":"committed","effects":[{"kind":"suggestion","id":"kept"}]}}),
        )
        .unwrap();

        SessionRegistry::recover_at_startup(dir.path());

        let recovered = State::load(&session_dir.join("runner.json")).unwrap();
        assert_eq!(recovered.tasks[0].status, TaskStatus::Interrupted);
        assert_eq!(recovered.tasks[0].results["suggestions"], json!(["kept"]));
    }

    /// Every stop path, including dashboard key-based stops, must reconcile
    /// committed journal effects before returning. Otherwise the aborted
    /// task's last durable result can be hidden until the next companion start.
    #[tokio::test]
    async fn stopping_by_key_recovers_interrupted_work_and_journal_receipts() {
        let dir = tempfile::tempdir().unwrap();
        let session_dir = dir.path().join("librepaper").join("assistant").join("s1");
        std::fs::create_dir_all(&session_dir).unwrap();
        let state_path = session_dir.join("runner.json");
        let mut state = State::default();
        let mut task =
            Task::from_message(&json!({"id":"one","role":"user","text":"Edit"})).unwrap();
        task.status = TaskStatus::Working;
        state.admit(task).unwrap();
        state.save(&state_path).unwrap();
        let journal_path = session_dir.join("runner.journal.json");
        let request = json!({"operation":{"epoch":"e","id":"suggest"}});
        crate::assistant::journal::record_tool_call(
            &journal_path,
            "one",
            "document_propose",
            &request,
        )
        .unwrap();
        crate::assistant::journal::record_tool_result(
            &journal_path,
            "one",
            "document_propose",
            &request,
            &json!({"result":{"status":"committed","effects":[{"kind":"suggestion","id":"kept"}]}}),
        )
        .unwrap();

        let registry = SessionRegistry::new(dir.path().to_path_buf());
        registry.spawn_running("s1", "hash", None, None, |_status, _stop| async move {
            std::future::pending::<()>().await;
            Ok(())
        });
        registry.stop_by_key("s1").await.unwrap();

        let recovered = State::load(&state_path).unwrap();
        assert_eq!(recovered.tasks[0].status, TaskStatus::Interrupted);
        assert_eq!(recovered.tasks[0].results["suggestions"], json!(["kept"]));
    }
}
