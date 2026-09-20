use tokio::sync::{mpsc, oneshot, Mutex, MutexGuard};

use super::RoomState;

/// One bounded Tokio task orders semantic work for a resident document.
pub(super) struct DocumentOwner {
    tx: mpsc::Sender<Request>,
    state: Mutex<RoomState>,
}

struct Request {
    granted: oneshot::Sender<()>,
    released: oneshot::Receiver<()>,
}

pub(crate) struct CommandLease {
    released: Option<oneshot::Sender<()>>,
}

impl DocumentOwner {
    pub(super) fn spawn(state: RoomState) -> Self {
        let (tx, mut rx) = mpsc::channel::<Request>(128);
        tokio::spawn(async move {
            while let Some(request) = rx.recv().await {
                if request.granted.send(()).is_err() {
                    continue;
                }
                let _ = request.released.await;
            }
        });
        Self {
            tx,
            state: Mutex::new(state),
        }
    }

    /// The mutable document projection is private to the owner abstraction.
    /// Semantic mutation callers must additionally hold a [`CommandLease`];
    /// read-only and ephemeral subscriber operations use this accessor so no
    /// adapter can obtain or replace the state container itself.
    pub(super) async fn state(&self) -> MutexGuard<'_, RoomState> {
        self.state.lock().await
    }

    pub(super) fn try_state(&self) -> Option<MutexGuard<'_, RoomState>> {
        self.state.try_lock().ok()
    }

    pub(super) async fn acquire(&self) -> CommandLease {
        let (granted_tx, granted_rx) = oneshot::channel();
        let (released_tx, released_rx) = oneshot::channel();
        self.tx
            .send(Request {
                granted: granted_tx,
                released: released_rx,
            })
            .await
            .expect("document owner task lives as long as its room");
        granted_rx
            .await
            .expect("document owner task grants or retains every request");
        CommandLease {
            released: Some(released_tx),
        }
    }
}

impl Drop for CommandLease {
    fn drop(&mut self) {
        if let Some(released) = self.released.take() {
            let _ = released.send(());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{DocumentOwner, RoomState};
    use crate::document::session;
    use crate::room::{Attribution, Manifest, Measured, Session};
    use std::collections::HashMap;
    use std::time::Duration;

    fn owner() -> DocumentOwner {
        DocumentOwner::spawn(RoomState {
            seq: 0,
            comments: Measured::new(Vec::new()),
            sockets: HashMap::new(),
            rate: HashMap::new(),
            session: Session {
                doc: session::new_doc(),
                durable_sequence: 0,
                durable_vector: Default::default(),
                durable_state_bytes: 0,
                dirty: false,
                dirty_since: 0,
                last_persist_at: 0,
                generation: 0,
                encoded_size: None,
                checkpoint_tree: None,
                updated_at: 0,
                by: Attribution::system(),
                format: String::new(),
                asset_sizes: HashMap::new(),
            },
            manifest: Measured::new(Manifest::default()),
            touched: 0,
        })
    }

    #[tokio::test]
    async fn grants_exactly_one_command_at_a_time() {
        let owner = owner();
        let first = owner.acquire().await;
        let second = owner.acquire();
        tokio::pin!(second);

        assert!(tokio::time::timeout(Duration::from_millis(20), &mut second)
            .await
            .is_err());
        drop(first);
        tokio::time::timeout(Duration::from_secs(1), &mut second)
            .await
            .expect("next command is granted after release");
    }

    #[tokio::test]
    async fn cancelling_a_waiter_does_not_block_following_commands() {
        let owner = owner();
        let first = owner.acquire().await;
        let mut waiting = Box::pin(owner.acquire());
        assert!(
            tokio::time::timeout(Duration::from_millis(20), &mut waiting)
                .await
                .is_err()
        );
        drop(waiting);
        drop(first);

        tokio::time::timeout(Duration::from_secs(1), owner.acquire())
            .await
            .expect("cancelled request is skipped");
    }
}
