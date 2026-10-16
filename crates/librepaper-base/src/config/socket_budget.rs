//! Admission and rolling budgets for live collaboration transports.
//!
//! The room owns document state; this process-wide meter owns the resources
//! shared by rooms.  Keeping the counters here makes assistant and companion
//! sockets consume the same deployment allowance as ordinary room sockets.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use serde::Serialize;
use serde_json::{json, Value};

/// Live sockets a deployment admits per MiB of `memory_budget_bytes`: 4096
/// at the default 512 MiB. Operators size one number, the memory budget, and
/// the socket ceiling follows it.
pub const SOCKETS_PER_MIB: usize = 8;

/// The deployment socket ceiling for a memory budget, never below 64.
pub fn deployment_sockets(memory_budget_bytes: u64) -> usize {
    ((memory_budget_bytes >> 20) as usize)
        .saturating_mul(SOCKETS_PER_MIB)
        .max(64)
}

/// Live socket guardrails.  Configuration can replace this value at server
/// construction without changing the accounting algorithm.
///
/// A whole lecture hall can share one public address, so there is no
/// per-network cap: every socket belongs to a signed-in account, the
/// per-account cap bounds one person or agent, and the deployment cap bounds
/// everyone.
#[derive(Clone, Copy, Debug, Serialize)]
pub struct SocketPolicy {
    pub deployment_max: usize,
    pub principal_max: usize,
    pub document_max: usize,
    pub document_editors_max: usize,
    pub queue_bytes_max: usize,
    pub idle_seconds: u64,
}

impl Default for SocketPolicy {
    fn default() -> Self {
        Self {
            deployment_max: deployment_sockets(512 << 20),
            principal_max: 64,
            document_max: 256,
            document_editors_max: 32,
            queue_bytes_max: 16 * 1024 * 1024,
            idle_seconds: 30 * 60,
        }
    }
}

/// Effective document access at the room handshake. Owners use editor slots.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SocketRole {
    Reader,
    Commenter,
    Editor,
}

impl SocketRole {
    /// The per-document cap for this role, if it has one. Only editors do:
    /// readers and commenters are bound by the document ceiling alone.
    fn limit(self, policy: &SocketPolicy) -> Option<usize> {
        match self {
            Self::Editor => Some(policy.document_editors_max),
            Self::Reader | Self::Commenter => None,
        }
    }
}

#[derive(Clone, Debug)]
pub struct SocketIdentity {
    pub principal: String,
    pub document: String,
    /// Control/chat transports use only the overall socket allowances.
    pub role: Option<SocketRole>,
}

#[derive(Default)]
struct DocumentSockets {
    total: usize,
    roles: [usize; 3],
}

struct State {
    active: HashMap<u64, SocketIdentity>,
    principals: HashMap<String, usize>,
    documents: HashMap<String, DocumentSockets>,
    queue_frames: usize,
    queue_bytes: usize,
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
                principals: HashMap::new(),
                documents: HashMap::new(),
                queue_frames: 0,
                queue_bytes: 0,
            }),
        })
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.state
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
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
        let principal = if identity.principal.is_empty() {
            "anonymous"
        } else {
            &identity.principal
        };
        if state.principals.get(principal).copied().unwrap_or(0) >= self.policy.principal_max {
            return Err(Refusal::Principal);
        }
        let document = state.documents.get(&identity.document);
        if document.map_or(0, |counts| counts.total) >= self.policy.document_max {
            return Err(Refusal::Document);
        }
        // Only the editor role has a cap, so only an editor can be refused here.
        if let Some(role) = identity.role {
            if role.limit(&self.policy).is_some_and(|limit| {
                document.map_or(0, |counts| counts.roles[role as usize]) >= limit
            }) {
                return Err(Refusal::DocumentEditors);
            }
        }
        *state.principals.entry(principal.to_string()).or_default() += 1;
        let document = state
            .documents
            .entry(identity.document.clone())
            .or_default();
        document.total += 1;
        if let Some(role) = identity.role {
            document.roles[role as usize] += 1;
        }
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
        decrement(
            &mut state.principals,
            if identity.principal.is_empty() {
                "anonymous"
            } else {
                &identity.principal
            },
        );
        if let Some(document) = state.documents.get_mut(&identity.document) {
            document.total -= 1;
            if let Some(role) = identity.role {
                document.roles[role as usize] -= 1;
            }
            if document.total == 0 {
                state.documents.remove(&identity.document);
            }
        }
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
        let state = self.lock();
        json!({
            "active": state.active.len(),
            "principals": state.principals.len(),
            "documents": state.documents.len(),
            "max_principal_sockets": state.principals.values().copied().max().unwrap_or(0),
            "max_document_sockets": state.documents.values().map(|counts| counts.total).max().unwrap_or(0),
            "max_document_reader_sockets": state.documents.values().map(|counts| counts.roles[SocketRole::Reader as usize]).max().unwrap_or(0),
            "max_document_commenter_sockets": state.documents.values().map(|counts| counts.roles[SocketRole::Commenter as usize]).max().unwrap_or(0),
            "max_document_editor_sockets": state.documents.values().map(|counts| counts.roles[SocketRole::Editor as usize]).max().unwrap_or(0),
            "queue_frames": state.queue_frames,
            "queue_bytes": state.queue_bytes,
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
    Principal,
    Document,
    DocumentEditors,
}

impl Refusal {
    pub fn scope(self) -> &'static str {
        match self {
            Self::Deployment => "deployment",
            Self::Principal => "principal",
            Self::Document => "document",
            Self::DocumentEditors => "document_editors",
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

    fn identity(principal: &str, document: &str, role: Option<SocketRole>) -> SocketIdentity {
        SocketIdentity {
            principal: principal.into(),
            document: document.into(),
            role,
        }
    }

    #[test]
    fn lecture_hall_readers_from_one_address_are_all_admitted() {
        let budget = SocketBudget::new(SocketPolicy::default());
        let guards: Vec<_> = (0..150)
            .map(|id| {
                budget
                    .admit(
                        id,
                        identity(&format!("user{id}"), "d", Some(SocketRole::Reader)),
                    )
                    .unwrap()
            })
            .collect();
        assert_eq!(budget.snapshot()["active"], 150);
        drop(guards);
        assert_eq!(budget.snapshot()["active"], 0);
    }

    #[test]
    fn the_33rd_editor_is_refused_and_a_reader_is_still_admitted() {
        let budget = SocketBudget::new(SocketPolicy::default());
        let mut guards = Vec::new();
        for id in 0..32 {
            guards.push(
                budget
                    .admit(
                        id,
                        identity(&format!("editor{id}"), "d", Some(SocketRole::Editor)),
                    )
                    .unwrap(),
            );
        }
        assert_eq!(
            budget
                .admit(32, identity("editor32", "d", Some(SocketRole::Editor)))
                .err(),
            Some(Refusal::DocumentEditors)
        );
        assert_eq!(budget.snapshot()["max_document_editor_sockets"], 32);
        guards.push(
            budget
                .admit(33, identity("reader", "d", Some(SocketRole::Reader)))
                .unwrap(),
        );
        drop(guards);
        assert_eq!(budget.snapshot()["active"], 0);
        assert_eq!(budget.snapshot()["documents"], 0);
    }

    #[test]
    fn the_65th_socket_for_one_principal_is_refused() {
        let budget = SocketBudget::new(SocketPolicy::default());
        let guards: Vec<_> = (0..64)
            .map(|id| {
                budget
                    .admit(id, identity("p", &format!("doc{id}"), None))
                    .unwrap()
            })
            .collect();
        assert_eq!(
            budget.admit(64, identity("p", "doc64", None)).err(),
            Some(Refusal::Principal)
        );
        drop(guards);
        assert_eq!(budget.snapshot()["active"], 0);
    }

    #[test]
    fn deployment_ceiling_follows_the_memory_budget() {
        assert_eq!(deployment_sockets(512 << 20), 4096);
        assert_eq!(deployment_sockets(1024 << 20), 8192);
        assert_eq!(deployment_sockets(1 << 20), 64);
        assert_eq!(SocketPolicy::default().deployment_max, 4096);
    }

    #[test]
    fn auxiliary_sockets_share_the_document_ceiling_without_using_role_slots() {
        let budget = SocketBudget::new(SocketPolicy {
            document_max: 2,
            document_editors_max: 1,
            ..Default::default()
        });
        let editor = budget
            .admit(1, identity("p", "d", Some(SocketRole::Editor)))
            .unwrap();
        let control = budget.admit(2, identity("p", "d", None)).unwrap();
        assert_eq!(budget.snapshot()["max_document_editor_sockets"], 1);
        assert_eq!(
            budget
                .admit(3, identity("p", "d", Some(SocketRole::Reader)))
                .err(),
            Some(Refusal::Document)
        );
        drop(control);
        let reader = budget
            .admit(3, identity("p", "d", Some(SocketRole::Reader)))
            .unwrap();
        drop((editor, reader));
        assert_eq!(budget.snapshot()["active"], 0);
    }

    #[test]
    fn concurrent_admission_cannot_exceed_a_role_cap() {
        let budget = SocketBudget::new(SocketPolicy {
            document_editors_max: 3,
            ..Default::default()
        });
        let start = Arc::new(std::sync::Barrier::new(12));
        let attempts: Vec<_> = (0..12)
            .map(|id| {
                let budget = budget.clone();
                let start = start.clone();
                std::thread::spawn(move || {
                    start.wait();
                    budget.admit(id, identity("p", "d", Some(SocketRole::Editor)))
                })
            })
            .collect();
        let results: Vec<_> = attempts
            .into_iter()
            .map(|thread| thread.join().unwrap())
            .collect();
        assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 3);
        assert!(results
            .iter()
            .filter_map(|result| result.as_ref().err())
            .all(|reason| *reason == Refusal::DocumentEditors));
        assert_eq!(budget.snapshot()["active"], 3);
        drop(results);
        assert_eq!(budget.snapshot()["active"], 0);
    }

    #[test]
    fn socket_admission_releases_counts_with_the_transport_guard() {
        let budget = SocketBudget::new(SocketPolicy {
            deployment_max: 1,
            ..Default::default()
        });
        let identity = identity("p", "d", Some(SocketRole::Editor));
        let guard = budget.admit(1, identity.clone()).unwrap();
        assert!(budget.admit(2, identity.clone()).is_err());
        drop(guard);
        assert!(budget.admit(2, identity).is_ok());
        assert_eq!(budget.snapshot()["active"], 0);
    }
}
