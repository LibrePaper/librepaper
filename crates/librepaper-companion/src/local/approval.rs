//! Per-service browser approval queue.
//!
//! Approval requests are deliberately owned by one running local service.
//! They are never kept in a process-global credential table, and the
//! six-digit CLI code is only indexed inside that same broker.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tokio::sync::oneshot;

const MAX_PENDING: usize = 64;
const DEFAULT_TTL: Duration = Duration::from_secs(300);

#[cfg(test)]
thread_local! {
    static SCRIPTED: std::cell::RefCell<Option<bool>> = const { std::cell::RefCell::new(None) };
}

#[derive(Clone, Debug)]
pub(crate) struct Approval {
    pub title: String,
    pub message: String,
    pub allow_label: String,
    /// Optional pairing/site scope so revocation can invalidate that pending
    /// consent without touching unrelated machine-local approvals.
    pub scope: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Decision {
    Allowed,
    Denied,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DecisionResult {
    Applied,
    Unknown,
    Expired,
}

#[derive(Clone, Debug, serde::Serialize)]
pub(crate) struct PendingView {
    pub id: String,
    pub title: String,
    pub message: String,
    pub allow_label: String,
    pub expires_in_seconds: u64,
}

struct Pending {
    approval: Approval,
    expires: Instant,
    code: String,
    answer: oneshot::Sender<Decision>,
}

#[derive(Default)]
struct State {
    pending: HashMap<String, Pending>,
}

pub(crate) type ApprovalOpener = Arc<dyn Fn() + Send + Sync>;

/// A service-local queue that wakes the waiting operation after one explicit
/// allow/deny decision. The queue is bounded and every wait removes its entry
/// on decision, timeout, or cancellation.
#[derive(Clone, Default)]
pub(crate) struct ApprovalBroker {
    state: Arc<Mutex<State>>,
    opener: Arc<Mutex<Option<ApprovalOpener>>>,
}

struct PendingGuard {
    broker: ApprovalBroker,
    id: String,
}

impl Drop for PendingGuard {
    fn drop(&mut self) {
        self.broker
            .state
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .pending
            .remove(&self.id);
    }
}

impl ApprovalBroker {
    pub(crate) async fn ask(&self, approval: &Approval, ttl: Duration) -> Decision {
        #[cfg(test)]
        if let Some(allowed) = SCRIPTED.with(|script| *script.borrow()) {
            return if allowed {
                Decision::Allowed
            } else {
                Decision::Denied
            };
        }

        let (answer, receiver) = oneshot::channel();
        let expires = Instant::now() + ttl.min(DEFAULT_TTL);
        let (id, code, should_open) = {
            let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
            reap_expired(&mut state);
            if state.pending.len() >= MAX_PENDING {
                return Decision::Denied;
            }
            let should_open = state.pending.is_empty();
            let id = fresh_id(&state);
            let code = fresh_code(&state);
            state.pending.insert(
                id.clone(),
                Pending {
                    approval: approval.clone(),
                    expires,
                    code: code.clone(),
                    answer,
                },
            );
            (id, code, should_open)
        };
        let _cleanup = PendingGuard {
            broker: self.clone(),
            id: id.clone(),
        };

        eprintln!(
            "LibrePaper approval pending: {}. Approve in the companion panel or run: librepaper local approve {}",
            approval.title, code
        );
        if should_open && browser_available() {
            if let Some(open) = self
                .opener
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .clone()
            {
                open();
            }
        }

        let decision = match tokio::time::timeout_at(
            tokio::time::Instant::from_std(expires),
            receiver,
        )
        .await
        {
            Ok(Ok(decision)) => decision,
            _ => Decision::Denied,
        };
        let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        state.pending.remove(&id);
        decision
    }

    pub(crate) fn set_opener(&self, opener: Option<ApprovalOpener>) {
        *self
            .opener
            .lock()
            .unwrap_or_else(|error| error.into_inner()) = opener;
    }

    pub(crate) fn list(&self) -> Vec<PendingView> {
        let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        state
            .pending
            .retain(|_, pending| !pending.answer.is_closed());
        reap_expired(&mut state);
        let now = Instant::now();
        let mut pending: Vec<_> = state
            .pending
            .iter()
            .map(|(id, entry)| PendingView {
                id: id.clone(),
                title: entry.approval.title.clone(),
                message: entry.approval.message.clone(),
                allow_label: entry.approval.allow_label.clone(),
                expires_in_seconds: entry.expires.saturating_duration_since(now).as_secs(),
            })
            .collect();
        pending.sort_by(|left, right| left.id.cmp(&right.id));
        pending
    }

    pub(crate) fn decide(&self, id: &str, decision: Decision) -> DecisionResult {
        let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        let Some(pending) = state.pending.remove(id) else {
            return DecisionResult::Unknown;
        };
        if pending.expires <= Instant::now() {
            return DecisionResult::Expired;
        }
        if pending.answer.send(decision).is_ok() {
            DecisionResult::Applied
        } else {
            DecisionResult::Unknown
        }
    }

    /// Called only by the loopback CLI approval route. Codes are never
    /// returned by the authenticated dashboard API.
    pub(crate) fn approve_code(&self, code: &str) -> bool {
        let code = code.trim();
        let id = {
            let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
            reap_expired(&mut state);
            state
                .pending
                .iter()
                .find(|(_, pending)| pending.code == code)
                .map(|(id, _)| id.clone())
        };
        id.is_some_and(|id| self.decide(&id, Decision::Allowed) == DecisionResult::Applied)
    }

    #[cfg(test)]
    pub(crate) fn codes_for_test(&self) -> Vec<String> {
        self.state
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .pending
            .values()
            .map(|pending| pending.code.clone())
            .collect()
    }

    pub(crate) fn deny_all(&self) {
        let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        for (_, pending) in state.pending.drain() {
            let _ = pending.answer.send(Decision::Denied);
        }
    }

    pub(crate) fn deny_scope(&self, scope: &str) {
        let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        let ids: Vec<_> = state
            .pending
            .iter()
            .filter_map(|(id, pending)| {
                pending
                    .approval
                    .scope
                    .as_deref()
                    .is_some_and(|approval_scope| {
                        crate::local::pairing::normalize_origin(approval_scope)
                            == crate::local::pairing::normalize_origin(scope)
                    })
                    .then_some(id.clone())
            })
            .collect();
        for id in ids {
            if let Some(pending) = state.pending.remove(&id) {
                let _ = pending.answer.send(Decision::Denied);
            }
        }
    }
}

fn browser_available() -> bool {
    #[cfg(target_os = "linux")]
    {
        std::env::var_os("DISPLAY").is_some() || std::env::var_os("WAYLAND_DISPLAY").is_some()
    }
    #[cfg(not(target_os = "linux"))]
    {
        true
    }
}

fn reap_expired(state: &mut State) {
    let now = Instant::now();
    state.pending.retain(|_, pending| pending.expires > now);
}

fn fresh_id(state: &State) -> String {
    loop {
        let candidate = hex::encode(librepaper_base::util::random_bytes(16));
        if !state.pending.contains_key(&candidate) {
            return candidate;
        }
    }
}

fn fresh_code(state: &State) -> String {
    use rand::Rng;
    loop {
        let code = format!("{:06}", rand::rng().random_range(0..1_000_000));
        if state.pending.values().all(|pending| pending.code != code) {
            return code;
        }
    }
}

#[cfg(test)]
pub(crate) fn script(allowed: bool) {
    SCRIPTED.with(|script| *script.borrow_mut() = Some(allowed));
}

#[cfg(test)]
pub(crate) fn unscript() {
    SCRIPTED.with(|script| *script.borrow_mut() = None);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn decisions_are_one_shot_and_scoped_to_the_broker() {
        let first = ApprovalBroker::default();
        let second = ApprovalBroker::default();
        let request = Approval {
            title: "Test".into(),
            message: "Allow?".into(),
            allow_label: "Allow".into(),
            scope: None,
        };
        let task_broker = first.clone();
        let task =
            tokio::spawn(async move { task_broker.ask(&request, Duration::from_secs(5)).await });
        tokio::task::yield_now().await;
        let pending = first.list();
        assert_eq!(pending.len(), 1);
        assert_eq!(
            second.decide(&pending[0].id, Decision::Allowed),
            DecisionResult::Unknown
        );
        assert_eq!(
            first.decide(&pending[0].id, Decision::Allowed),
            DecisionResult::Applied
        );
        assert_eq!(
            first.decide(&pending[0].id, Decision::Denied),
            DecisionResult::Unknown
        );
        assert_eq!(task.await.unwrap(), Decision::Allowed);
    }

    #[tokio::test]
    async fn expiry_and_cancellation_remove_pending_records() {
        let broker = ApprovalBroker::default();
        let request = Approval {
            title: "Test".into(),
            message: "Allow?".into(),
            allow_label: "Allow".into(),
            scope: None,
        };
        assert_eq!(
            broker.ask(&request, Duration::from_millis(1)).await,
            Decision::Denied
        );
        assert!(broker.list().is_empty());
        let task_broker = broker.clone();
        let task =
            tokio::spawn(async move { task_broker.ask(&request, Duration::from_secs(5)).await });
        tokio::task::yield_now().await;
        assert_eq!(broker.list().len(), 1);
        task.abort();
        let _ = task.await;
        assert!(broker.list().is_empty());
    }
}
