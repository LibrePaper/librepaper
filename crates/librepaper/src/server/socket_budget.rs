//! Admission and rolling budgets for live collaboration transports.
//!
//! The room owns document state; this process-wide meter owns the resources
//! shared by rooms.  Keeping the counters here makes assistant and companion
//! sockets consume the same deployment allowance as ordinary room sockets.

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::Serialize;
use serde_json::{json, Value};

// Small buckets make the allowance rolling instead of anchoring all charges
// to the first join in an hour. At most 61 buckets can be live.
const BUCKET: Duration = Duration::from_secs(60);
const WINDOW: Duration = Duration::from_secs(3600);
const MAX_NETWORK_KEYS: usize = 1024;
const OVERFLOW_NETWORK: &str = "\0other-networks";

/// Live socket guardrails.  Configuration can replace this value at server
/// construction without changing the accounting algorithm.
#[derive(Clone, Copy, Debug, Serialize)]
pub struct SocketPolicy {
    pub deployment_max: usize,
    pub network_max: usize,
    pub principal_max: usize,
    pub document_max: usize,
    pub queue_bytes_max: usize,
    pub state_network_bytes: u64,
    pub state_deployment_bytes: u64,
    pub idle_seconds: u64,
}

impl Default for SocketPolicy {
    fn default() -> Self {
        Self {
            deployment_max: 4096,
            network_max: 128,
            principal_max: 64,
            document_max: 256,
            queue_bytes_max: 16 * 1024 * 1024,
            state_network_bytes: 256 * 1024 * 1024,
            state_deployment_bytes: 4 * 1024 * 1024 * 1024,
            idle_seconds: 30 * 60,
        }
    }
}

#[derive(Clone, Debug)]
pub struct SocketIdentity {
    pub network: String,
    pub principal: String,
    pub document: String,
}

struct Window {
    expires: Instant,
    deployment: u64,
    networks: HashMap<String, u64>,
}

impl Default for Window {
    fn default() -> Self {
        Self {
            expires: Instant::now(),
            deployment: 0,
            networks: HashMap::new(),
        }
    }
}

struct State {
    active: HashMap<u64, SocketIdentity>,
    networks: HashMap<String, usize>,
    principals: HashMap<String, usize>,
    documents: HashMap<String, usize>,
    windows: VecDeque<Window>,
    queue_frames: usize,
    queue_bytes: usize,
    state_bytes: u64,
}

/// Process-local live socket accounting.  A deployment with multiple
/// workers should route all sockets to one process or share these counters at
/// the edge; the deployment cap still bounds every process independently.
pub struct SocketBudget {
    pub policy: SocketPolicy,
    state: Mutex<State>,
}

impl SocketBudget {
    pub fn new(policy: SocketPolicy) -> Arc<Self> {
        Arc::new(Self {
            policy,
            state: Mutex::new(State {
                active: HashMap::new(),
                networks: HashMap::new(),
                principals: HashMap::new(),
                documents: HashMap::new(),
                windows: VecDeque::new(),
                queue_frames: 0,
                queue_bytes: 0,
                state_bytes: 0,
            }),
        })
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.state
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
    }

    fn expire(state: &mut State) {
        let now = Instant::now();
        while state
            .windows
            .front()
            .is_some_and(|window| window.expires <= now)
        {
            state.windows.pop_front();
        }
    }

    fn network_usage(state: &State, network: &str) -> u64 {
        state
            .windows
            .iter()
            .map(|window| {
                let direct = window.networks.get(network).copied().unwrap_or(0);
                // Once a bucket's bounded breakdown is full, all unknown
                // networks share the overflow entry. Include it in the
                // admission check so identity churn cannot bypass the cap.
                direct.saturating_add(if direct == 0 {
                    window.networks.get(OVERFLOW_NETWORK).copied().unwrap_or(0)
                } else {
                    0
                })
            })
            .sum()
    }

    fn bucket_mut(state: &mut State, now: Instant) -> &mut Window {
        if state
            .windows
            .back()
            .is_none_or(|window| window.expires <= now + WINDOW)
        {
            state.windows.push_back(Window {
                expires: now + WINDOW + BUCKET,
                ..Window::default()
            });
        }
        state.windows.back_mut().expect("window just created")
    }

    fn charge_network(window: &mut Window, network: &str, bytes: u64) {
        let key =
            if window.networks.contains_key(network) || window.networks.len() < MAX_NETWORK_KEYS {
                network
            } else {
                OVERFLOW_NETWORK
            };
        let value = window.networks.entry(key.to_string()).or_default();
        *value = value.saturating_add(bytes);
    }

    /// Admit before loading a room.  The returned guard must be moved into
    /// the upgrade task and lives until the transport exits.
    pub fn admit(
        self: &Arc<Self>,
        id: u64,
        identity: SocketIdentity,
    ) -> Result<SocketPermit, Refusal> {
        let mut state = self.lock();
        if state.active.len() >= self.policy.deployment_max {
            return Err(Refusal::Deployment);
        }
        if state.networks.get(&identity.network).copied().unwrap_or(0) >= self.policy.network_max {
            return Err(Refusal::Network);
        }
        let principal = if identity.principal.is_empty() {
            "anonymous"
        } else {
            &identity.principal
        };
        if state.principals.get(principal).copied().unwrap_or(0) >= self.policy.principal_max {
            return Err(Refusal::Principal);
        }
        if state
            .documents
            .get(&identity.document)
            .copied()
            .unwrap_or(0)
            >= self.policy.document_max
        {
            return Err(Refusal::Document);
        }
        *state.networks.entry(identity.network.clone()).or_default() += 1;
        *state.principals.entry(principal.to_string()).or_default() += 1;
        *state
            .documents
            .entry(identity.document.clone())
            .or_default() += 1;
        state.active.insert(id, identity.clone());
        Ok(SocketPermit {
            budget: self.clone(),
            id,
            identity: Some(identity),
        })
    }

    fn release(&self, id: u64, identity: &SocketIdentity) {
        let mut state = self.lock();
        state.active.remove(&id);
        decrement(&mut state.networks, &identity.network);
        decrement(
            &mut state.principals,
            if identity.principal.is_empty() {
                "anonymous"
            } else {
                &identity.principal
            },
        );
        decrement(&mut state.documents, &identity.document);
    }

    /// Reserve one state transfer before encoding a room snapshot.  The
    /// actual size is charged after the locked snapshot has been produced;
    /// this initial check is what prevents a cold join from building state
    /// after the rolling allowance is already exhausted.
    pub fn state_available(&self, network: &str) -> bool {
        let mut state = self.lock();
        Self::expire(&mut state);
        let used = Self::network_usage(&state, network);
        let deployment = state
            .windows
            .iter()
            .map(|window| window.deployment)
            .sum::<u64>();
        used < self.policy.state_network_bytes && deployment < self.policy.state_deployment_bytes
    }

    pub fn charge_state(&self, network: &str, bytes: usize) -> bool {
        let mut state = self.lock();
        Self::expire(&mut state);
        let now = Instant::now();
        let deployment = state
            .windows
            .iter()
            .map(|window| window.deployment)
            .sum::<u64>();
        let network_used = Self::network_usage(&state, network);
        let bytes = bytes as u64;
        if network_used.saturating_add(bytes) > self.policy.state_network_bytes
            || deployment.saturating_add(bytes) > self.policy.state_deployment_bytes
        {
            return false;
        }
        let window = Self::bucket_mut(&mut state, now);
        window.deployment = window.deployment.saturating_add(bytes);
        Self::charge_network(window, network, bytes);
        state.state_bytes = state.state_bytes.saturating_add(bytes);
        true
    }

    pub fn queue_add(&self, frames: usize, bytes: usize) {
        let mut state = self.lock();
        state.queue_frames = state.queue_frames.saturating_add(frames);
        state.queue_bytes = state.queue_bytes.saturating_add(bytes);
    }

    pub fn queue_admit(&self, bytes: usize) -> bool {
        let mut state = self.lock();
        if state.queue_bytes.saturating_add(bytes) > self.policy.queue_bytes_max {
            return false;
        }
        state.queue_frames = state.queue_frames.saturating_add(1);
        state.queue_bytes = state.queue_bytes.saturating_add(bytes);
        true
    }

    pub fn queue_remove(&self, frames: usize, bytes: usize) {
        let mut state = self.lock();
        state.queue_frames = state.queue_frames.saturating_sub(frames);
        state.queue_bytes = state.queue_bytes.saturating_sub(bytes);
    }

    pub fn snapshot(&self) -> Value {
        let mut state = self.lock();
        Self::expire(&mut state);
        let network_state = state
            .windows
            .iter()
            .map(|window| window.networks.values().sum::<u64>())
            .sum::<u64>();
        let deployment_state = state
            .windows
            .iter()
            .map(|window| window.deployment)
            .sum::<u64>();
        json!({
            "active": state.active.len(),
            "networks": state.networks.len(),
            "principals": state.principals.len(),
            "documents": state.documents.len(),
            "max_network_sockets": state.networks.values().copied().max().unwrap_or(0),
            "max_principal_sockets": state.principals.values().copied().max().unwrap_or(0),
            "max_document_sockets": state.documents.values().copied().max().unwrap_or(0),
            "queue_frames": state.queue_frames,
            "queue_bytes": state.queue_bytes,
            "state_sync_bytes": state.state_bytes,
            "state_sync_window_bytes": deployment_state,
            "state_sync_network_window_bytes": network_state,
            "policy": self.policy,
        })
    }
}

fn decrement(values: &mut HashMap<String, usize>, key: &str) {
    if let Some(value) = values.get_mut(key) {
        *value = value.saturating_sub(1);
        if *value == 0 {
            values.remove(key);
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Refusal {
    Deployment,
    Network,
    Principal,
    Document,
}

impl Refusal {
    pub fn scope(self) -> &'static str {
        match self {
            Self::Deployment => "deployment",
            Self::Network => "network",
            Self::Principal => "principal",
            Self::Document => "document",
        }
    }
}

pub struct SocketPermit {
    budget: Arc<SocketBudget>,
    id: u64,
    identity: Option<SocketIdentity>,
}

impl SocketPermit {
    pub fn id(&self) -> u64 {
        self.id
    }
}

impl Drop for SocketPermit {
    fn drop(&mut self) {
        if let Some(identity) = self.identity.take() {
            self.budget.release(self.id, &identity);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn late_state_transfers_keep_a_full_rolling_hour() {
        let budget = SocketBudget::new(SocketPolicy {
            state_network_bytes: 10,
            state_deployment_bytes: 20,
            ..Default::default()
        });
        assert!(budget.charge_state("network", 4));
        {
            budget.lock().windows[0].expires = Instant::now() + WINDOW - Duration::from_secs(1);
        }
        assert!(budget.charge_state("network", 5));
        assert_eq!(budget.lock().windows.len(), 2);
        assert!(!budget.charge_state("network", 2));
        {
            budget.lock().windows[0].expires = Instant::now() - Duration::from_secs(1);
        }
        assert!(budget.charge_state("network", 5));
        assert!(!budget.charge_state("network", 1));
    }

    #[test]
    fn socket_admission_releases_counts_with_the_transport_guard() {
        let budget = SocketBudget::new(SocketPolicy {
            deployment_max: 1,
            ..Default::default()
        });
        let identity = SocketIdentity {
            network: "n".into(),
            principal: "p".into(),
            document: "d".into(),
        };
        let guard = budget.admit(1, identity.clone()).unwrap();
        assert!(budget.admit(2, identity.clone()).is_err());
        drop(guard);
        assert!(budget.admit(2, identity).is_ok());
        assert_eq!(budget.snapshot()["active"], 0);
    }

    #[test]
    fn reconnect_identity_churn_stays_bounded() {
        let budget = SocketBudget::new(SocketPolicy {
            state_network_bytes: 2,
            state_deployment_bytes: 10_000,
            ..Default::default()
        });
        for index in 0..MAX_NETWORK_KEYS + 2 {
            assert!(budget.charge_state(&index.to_string(), 1));
        }
        assert!(!budget.charge_state("another-identity", 1));
        assert_eq!(
            budget.lock().windows[0].networks.len(),
            MAX_NETWORK_KEYS + 1
        );
    }
}
