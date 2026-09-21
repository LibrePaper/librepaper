//! Immediate admission: no waiters, and only active principals occupy memory.
use super::{Failure, Viewer};
use serde_json::{json, Value};
use std::{collections::HashMap, sync::Mutex};

// Reads, effects, result lookups, cancellations. Keep control lanes separate
// so a caller can inspect or cancel its work while its work lanes are full.
const GLOBAL: [usize; 4] = [8, 8, 4, 2];
const PER_PRINCIPAL: [usize; 4] = [2, 2, 1, 1];

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
enum Principal {
    Account(String),
    Link(String),
}

impl Principal {
    fn of(who: &Viewer) -> Self {
        // Account IDs are catalogue identities. Do not include the document,
        // session, operation, attribution, or caller-controlled execution epoch.
        if who.id.is_signed_in() {
            Self::Account(who.id.id.clone())
        } else {
            // Viewer contains only a verified live link's digest. Anonymous
            // holders of the same capability deliberately share one allowance.
            Self::Link(who.link.clone())
        }
    }
}

#[derive(Default)]
struct State {
    active: [usize; 4],
    principals: HashMap<Principal, [usize; 4]>,
}

#[derive(Default)]
pub(in crate::server) struct Capacity {
    state: Mutex<State>,
}

pub(super) struct Permit<'a> {
    capacity: &'a Capacity,
    principal: Principal,
    lane: usize,
}

impl Capacity {
    pub(super) fn acquire(
        &self,
        who: &Viewer,
        name: &str,
        args: &Value,
    ) -> Result<Permit<'_>, Failure> {
        let lane = match name {
            "document_read" => 0,
            "document_result" if args["action"] == "cancel" => 3,
            "document_result" => 2,
            _ => 1,
        };
        let principal = Principal::of(who);
        // No await or tool execution under this short accounting lock. Check
        // both bounds atomically, without reserving a global slot for a refused
        // principal. At most sum(GLOBAL) = 22 entries can exist in the map.
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let personal = state.principals.get(&principal).map_or(0, |n| n[lane]);
        let scope = if personal >= PER_PRINCIPAL[lane] {
            Some("principal")
        } else if state.active[lane] >= GLOBAL[lane] {
            Some("deployment")
        } else {
            None
        };
        if let Some(scope) = scope {
            return Err(Failure::new(
                "rate_limited",
                "document tool capacity is busy; retry with the same operation key",
            )
            .with_data(json!({"retry_after_ms":250,"scope":scope})));
        }
        state.active[lane] += 1;
        state.principals.entry(principal.clone()).or_default()[lane] += 1;
        Ok(Permit {
            capacity: self,
            principal,
            lane,
        })
    }
}

impl Drop for Permit<'_> {
    fn drop(&mut self) {
        let mut state = self
            .capacity
            .state
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        state.active[self.lane] -= 1;
        if let Some(counts) = state.principals.get_mut(&self.principal) {
            counts[self.lane] -= 1;
            if *counts == [0; 4] {
                state.principals.remove(&self.principal);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{auth::Identity, server::Role};

    fn viewer(account: &str, link: &str) -> Viewer {
        Viewer {
            id: Identity {
                id: account.into(),
                ..Identity::default()
            },
            key: String::new(),
            link: link.into(),
            comment_budget: None,
            role: Role::Commenter,
            automation: true,
            auth_failed: false,
        }
    }

    const LANES: [(&str, &str); 4] = [
        ("document_read", ""),
        ("document_apply", ""),
        ("document_result", "status"),
        ("document_result", "cancel"),
    ];

    #[test]
    fn saturated_principal_leaves_room_for_another_in_every_lane() {
        for (lane, (name, action)) in LANES.iter().enumerate() {
            let capacity = Capacity::default();
            let args = json!({"action":action});
            let alice = viewer("alice", "link-a");
            let bob = viewer("bob", "link-b");
            let permits: Vec<_> = (0..PER_PRINCIPAL[lane])
                .map(|_| capacity.acquire(&alice, name, &args).unwrap())
                .collect();
            let error = capacity.acquire(&alice, name, &args).err().unwrap();
            assert_eq!(error.code, "rate_limited");
            assert_eq!(
                error.data,
                json!({"retry_after_ms":250,"scope":"principal"})
            );
            let other = capacity.acquire(&bob, name, &args).unwrap();
            drop(permits);
            let replacement = capacity.acquire(&alice, name, &args).unwrap();
            drop((other, replacement));
            let state = capacity.state.lock().unwrap();
            assert!(state.principals.is_empty());
            assert_eq!(state.active, [0; 4]);
        }
    }

    #[test]
    fn global_limits_remain_and_rejections_do_not_leak_entries() {
        for (lane, (name, action)) in LANES.iter().enumerate() {
            let capacity = Capacity::default();
            let args = json!({"action":action});
            let mut permits: Vec<_> = (0..GLOBAL[lane])
                .map(|i| {
                    capacity
                        .acquire(&viewer(&i.to_string(), ""), name, &args)
                        .unwrap()
                })
                .collect();
            for i in 0..100 {
                let error = capacity
                    .acquire(&viewer(&format!("refused-{i}"), ""), name, &args)
                    .err()
                    .unwrap();
                assert_eq!(error.data["scope"], "deployment");
            }
            assert_eq!(
                capacity.state.lock().unwrap().principals.len(),
                GLOBAL[lane]
            );
            permits.pop();
            let recovered = capacity.acquire(&viewer("new", ""), name, &args).unwrap();
            drop((permits, recovered));
            assert!(capacity.state.lock().unwrap().principals.is_empty());
        }
    }

    #[test]
    fn sessions_links_and_attribution_do_not_split_an_account_allowance() {
        let capacity = Capacity::default();
        let alice = viewer("alice", "link-a");
        let mut another_session = viewer("alice", "link-b");
        another_session.id.session_generation = "another-session".into();
        another_session.id.name = "another-name".into();
        another_session.key = "another-key".into();
        another_session.role = Role::Editor;
        let _one = capacity
            .acquire(&alice, "document_read", &Value::Null)
            .unwrap();
        let _two = capacity
            .acquire(&another_session, "document_read", &Value::Null)
            .unwrap();
        assert!(capacity
            .acquire(&alice, "document_read", &Value::Null)
            .is_err());
        assert_eq!(capacity.state.lock().unwrap().principals.len(), 1);
    }

    #[test]
    fn anonymous_link_holders_share_capacity_and_accounts_have_a_separate_namespace() {
        let capacity = Capacity::default();
        let guest = viewer("", "shared");
        let mut other_guest = guest.clone();
        other_guest.key = "different-visitor".into();
        let _one = capacity
            .acquire(&guest, "document_read", &Value::Null)
            .unwrap();
        let _two = capacity
            .acquire(&other_guest, "document_read", &Value::Null)
            .unwrap();
        assert!(capacity
            .acquire(&other_guest, "document_read", &Value::Null)
            .is_err());
        let _different_link = capacity
            .acquire(&viewer("", "other"), "document_read", &Value::Null)
            .unwrap();
        let _account = capacity
            .acquire(&viewer("shared", ""), "document_read", &Value::Null)
            .unwrap();
    }

    #[test]
    fn work_saturation_preserves_result_and_cancellation_lanes() {
        let capacity = Capacity::default();
        let alice = viewer("alice", "");
        let mut permits = Vec::new();
        for (name, action) in &LANES[..2] {
            for _ in 0..2 {
                permits.push(
                    capacity
                        .acquire(&alice, name, &json!({"action":action}))
                        .unwrap(),
                );
            }
        }
        let _result = capacity
            .acquire(&alice, "document_result", &json!({"action":"status"}))
            .unwrap();
        let _cancel = capacity
            .acquire(&alice, "document_result", &json!({"action":"cancel"}))
            .unwrap();
    }

    #[tokio::test]
    async fn aborting_a_request_releases_all_accounting() {
        let capacity = std::sync::Arc::new(Capacity::default());
        let task_capacity = capacity.clone();
        let (ready, started) = tokio::sync::oneshot::channel();
        let task = tokio::spawn(async move {
            let _permit = task_capacity
                .acquire(&viewer("alice", ""), "document_result", &Value::Null)
                .unwrap();
            ready.send(()).unwrap();
            std::future::pending::<()>().await;
        });
        started.await.unwrap();
        assert!(capacity
            .acquire(&viewer("alice", ""), "document_result", &Value::Null)
            .is_err());
        task.abort();
        assert!(task.await.unwrap_err().is_cancelled());
        assert!(capacity.state.lock().unwrap().principals.is_empty());
        let _replacement = capacity
            .acquire(&viewer("alice", ""), "document_result", &Value::Null)
            .unwrap();
    }

    #[test]
    fn simultaneous_requests_cannot_overbook_a_principal() {
        let capacity = Capacity::default();
        let barrier = std::sync::Barrier::new(16);
        std::thread::scope(|scope| {
            let tasks: Vec<_> = (0..16)
                .map(|_| {
                    scope.spawn(|| {
                        barrier.wait();
                        let permit =
                            capacity.acquire(&viewer("alice", ""), "document_read", &Value::Null);
                        // Hold every admitted permit until all attempts finish.
                        barrier.wait();
                        permit.is_ok()
                    })
                })
                .collect();
            let admitted = tasks
                .into_iter()
                .map(|t| usize::from(t.join().unwrap()))
                .sum::<usize>();
            assert_eq!(admitted, 2);
        });
        assert!(capacity.state.lock().unwrap().principals.is_empty());
    }

    #[tokio::test]
    async fn overload_response_preserves_retry_contract() {
        let capacity = Capacity::default();
        let who = viewer("alice", "");
        let _held = capacity
            .acquire(&who, "document_result", &Value::Null)
            .unwrap();
        let error = capacity
            .acquire(&who, "document_result", &Value::Null)
            .err()
            .unwrap();
        let reply = super::super::tool_result(&json!(17), Err(error));
        assert_eq!(reply.status(), 200);
        let body = axum::body::to_bytes(reply.into_body(), 8192).await.unwrap();
        let value: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(value["id"], 17);
        assert_eq!(value["result"]["isError"], true);
        let error = &value["result"]["structuredContent"]["error"];
        assert_eq!(error["code"], "rate_limited");
        assert_eq!(
            error["recovery"],
            json!({"retry_after_ms":250,"scope":"principal"})
        );
    }
}
