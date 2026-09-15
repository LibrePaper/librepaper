//! Bounded delivery queues shared by document and assistant sockets.
//!
//! A queue reservation belongs to the queued item itself. Dropping a
//! receiver, aborting a writer, or failing an enqueue therefore releases the
//! exact reservation which was made, without a racy queue-wide reset.

use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};

use axum::extract::ws::Utf8Bytes;
use tokio::sync::mpsc;

/// What a room or assistant channel sends to one connected socket.
#[derive(Clone, Debug)]
pub enum Outgoing {
    Text(String),
    /// Text serialized once for a fanout. `Utf8Bytes` is clone-on-reference, so
    /// every queue item and the websocket handoff can share the same payload
    /// allocation.
    SharedText(Utf8Bytes),
    Close(String),
}

impl Outgoing {
    /// Build a text frame whose immutable payload can be cheaply cloned for
    /// each recipient.
    pub fn shared_text(value: impl Into<Utf8Bytes>) -> Self {
        Self::SharedText(value.into())
    }

    /// Approximate wire bytes conservatively. JSON text is UTF-8 and the
    /// framing allowance covers a small WebSocket header.
    pub fn bytes(&self) -> usize {
        match self {
            Self::Text(value) => value.len().saturating_add(2),
            Self::SharedText(value) => value.len().saturating_add(2),
            Self::Close(reason) => reason.len().saturating_add(2),
        }
    }
}

#[derive(Default)]
struct QueueCounts {
    frames: usize,
    bytes: usize,
}

struct QueueState {
    counts: Mutex<QueueCounts>,
    frame_limit: usize,
    byte_limit: usize,
    enabled: bool,
}

impl QueueState {
    fn reserve(&self, bytes: usize) -> bool {
        if !self.enabled {
            return true;
        }
        let mut counts = self
            .counts
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        let Some(next_bytes) = counts.bytes.checked_add(bytes) else {
            return false;
        };
        if counts.frames >= self.frame_limit || next_bytes > self.byte_limit {
            return false;
        }
        counts.frames += 1;
        counts.bytes = next_bytes;
        true
    }

    fn release(&self, bytes: usize) {
        if !self.enabled {
            return;
        }
        let mut counts = self
            .counts
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        // A reservation is released at most once by its RAII guard. Saturating
        // arithmetic keeps malformed metadata from poisoning future admission.
        counts.frames = counts.frames.saturating_sub(1);
        counts.bytes = counts.bytes.saturating_sub(bytes);
    }

    #[cfg(test)]
    fn snapshot(&self) -> (usize, usize) {
        let counts = self
            .counts
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        (counts.frames, counts.bytes)
    }
}

/// A queued frame. The reservation is released when this value is dropped,
/// including when a writer task is aborted while it owns the item.
pub struct QueuedOutgoing {
    outgoing: Outgoing,
    durability: bool,
    reservation: Option<Reservation>,
}

impl QueuedOutgoing {
    pub fn durability(&self) -> bool {
        self.durability
    }

    pub fn into_parts(self) -> (Outgoing, Option<Reservation>) {
        (self.outgoing, self.reservation)
    }
}

pub struct Reservation {
    queue: Arc<QueueState>,
    bytes: usize,
    queue_metric: Option<Arc<dyn Fn(isize, usize) + Send + Sync>>,
}

impl Drop for Reservation {
    fn drop(&mut self) {
        self.queue.release(self.bytes);
        if let Some(metric) = &self.queue_metric {
            metric(-1, self.bytes);
        }
    }
}

enum Inner {
    Bounded(mpsc::Sender<QueuedOutgoing>),
    #[cfg(test)]
    Raw(mpsc::Sender<Outgoing>),
}

/// A production queue receiver. Each item carries its own RAII reservation
/// and durability bit.
pub type Receiver = mpsc::Receiver<QueuedOutgoing>;

/// A cloneable, byte-bounded sender. `send` has no producer wait: callers
/// either enqueue immediately or get an error and disconnect a slow peer.
#[derive(Clone)]
pub struct Sender {
    inner: Arc<Inner>,
    queue: Arc<QueueState>,
    queue_metric: Option<Arc<dyn Fn(isize, usize) + Send + Sync>>,
    queue_admission: Option<Arc<dyn Fn(usize) -> bool + Send + Sync>>,
}

impl Sender {
    pub fn channel(
        frame_limit: usize,
        byte_limit: usize,
        queue_metric: Option<Arc<dyn Fn(isize, usize) + Send + Sync>>,
        queue_admission: Option<Arc<dyn Fn(usize) -> bool + Send + Sync>>,
    ) -> (Self, Receiver) {
        let (inner, receiver) = mpsc::channel(frame_limit.max(1));
        (
            Self {
                inner: Arc::new(Inner::Bounded(inner)),
                queue: Arc::new(QueueState {
                    counts: Mutex::new(QueueCounts::default()),
                    frame_limit: frame_limit.max(1),
                    byte_limit: byte_limit.max(1),
                    enabled: true,
                }),
                queue_metric,
                queue_admission,
            },
            receiver,
        )
    }

    /// Adapt raw channels used by room unit tests and small internal callers.
    /// They retain Tokio's original semantics and have no budget.
    #[cfg(test)]
    pub fn from_raw(inner: mpsc::Sender<Outgoing>) -> Self {
        Self {
            inner: Arc::new(Inner::Raw(inner)),
            queue: Arc::new(QueueState {
                counts: Mutex::new(QueueCounts::default()),
                frame_limit: usize::MAX,
                byte_limit: usize::MAX,
                enabled: false,
            }),
            queue_metric: None,
            queue_admission: None,
        }
    }

    #[cfg(test)]
    pub fn queued(&self) -> (usize, usize) {
        self.queue.snapshot()
    }

    pub fn try_send(&self, outgoing: Outgoing) -> Result<(), Outgoing> {
        self.try_send_kind(outgoing, false)
    }

    pub fn try_send_durable(&self, outgoing: Outgoing) -> Result<(), Outgoing> {
        self.try_send_kind(outgoing, true)
    }

    fn try_send_kind(&self, outgoing: Outgoing, durability: bool) -> Result<(), Outgoing> {
        let bytes = outgoing.bytes();
        #[cfg(not(test))]
        let Inner::Bounded(inner) = self.inner.as_ref();
        #[cfg(test)]
        let inner = match self.inner.as_ref() {
            Inner::Bounded(inner) => inner,
            #[cfg(test)]
            Inner::Raw(inner) => {
                // Raw channels predate the bounded sender and are used by
                // room tests. Keep their observable String representation so
                // those tests do not need to know about the production-only
                // sharing optimization.
                let outgoing = match outgoing {
                    Outgoing::SharedText(text) => Outgoing::Text(text.to_string()),
                    outgoing => outgoing,
                };
                return inner.try_send(outgoing).map_err(|error| error.into_inner());
            }
        };
        if !self.queue.reserve(bytes) {
            return Err(outgoing);
        }
        if self
            .queue_admission
            .as_ref()
            .is_some_and(|admit| !admit(bytes))
        {
            self.queue.release(bytes);
            return Err(outgoing);
        }
        if self.queue_admission.is_none() {
            if let Some(metric) = &self.queue_metric {
                metric(1, bytes);
            }
        }
        let reservation = Reservation {
            queue: self.queue.clone(),
            bytes,
            queue_metric: self.queue_metric.clone(),
        };
        let queued = QueuedOutgoing {
            outgoing,
            durability,
            reservation: Some(reservation),
        };
        if let Err(error) = inner.try_send(queued) {
            let queued = error.into_inner();
            return Err(queued.outgoing);
        }
        Ok(())
    }

    /// Deliberately immediate and bounded; the async shape is retained for
    /// transport call sites which apply a write timeout.
    pub async fn send(&self, outgoing: Outgoing) -> Result<(), Outgoing> {
        self.try_send(outgoing)
    }
}

#[cfg(test)]
impl From<mpsc::Sender<Outgoing>> for Sender {
    fn from(value: mpsc::Sender<Outgoing>) -> Self {
        Self::from_raw(value)
    }
}

/// An outbound sink used by the bounded socket timeout helper.
pub trait OutgoingSink {
    fn send_boxed<'a>(
        &'a self,
        outgoing: Outgoing,
    ) -> Pin<Box<dyn Future<Output = Result<(), ()>> + Send + 'a>>;
}

impl OutgoingSink for Sender {
    fn send_boxed<'a>(
        &'a self,
        outgoing: Outgoing,
    ) -> Pin<Box<dyn Future<Output = Result<(), ()>> + Send + 'a>> {
        Box::pin(async move { self.send(outgoing).await.map_err(|_| ()) })
    }
}

#[cfg(test)]
impl OutgoingSink for mpsc::Sender<Outgoing> {
    fn send_boxed<'a>(
        &'a self,
        outgoing: Outgoing,
    ) -> Pin<Box<dyn Future<Output = Result<(), ()>> + Send + 'a>> {
        Box::pin(async move { self.send(outgoing).await.map_err(|_| ()) })
    }
}

#[cfg(test)]
mod budget_tests {
    use super::*;

    #[tokio::test]
    async fn queued_and_in_flight_bytes_release_on_every_drop() {
        let (sender, mut receiver) = Sender::channel(2, 12, None, None);
        sender.try_send(Outgoing::Text("1234".into())).unwrap();
        sender
            .try_send_durable(Outgoing::Text("5678".into()))
            .unwrap();
        assert!(sender.try_send(Outgoing::Text("x".into())).is_err());
        let first = receiver.recv().await.unwrap();
        let (_body, in_flight) = first.into_parts();
        assert_eq!(sender.queued(), (2, 12));
        drop(receiver);
        assert_eq!(sender.queued(), (1, 6));
        drop(in_flight);
        assert_eq!(sender.queued(), (0, 0));
        assert!(sender.try_send(Outgoing::Text("cancelled".into())).is_err());
        assert_eq!(sender.queued(), (0, 0));
    }

    #[tokio::test]
    async fn aggregate_queue_is_reserved_once_and_released_when_receiver_drops() {
        let budget = crate::server::socket_budget::SocketBudget::new(
            crate::server::socket_budget::SocketPolicy {
                queue_bytes_max: 6,
                ..Default::default()
            },
        );
        let admit = budget.clone();
        let release = budget.clone();
        let (sender, receiver) = Sender::channel(
            10,
            100,
            Some(Arc::new(move |frames, bytes| {
                if frames < 0 {
                    release.queue_remove((-frames) as usize, bytes);
                } else {
                    release.queue_add(frames as usize, bytes);
                }
            })),
            Some(Arc::new(move |bytes| admit.queue_admit(bytes))),
        );
        sender.try_send(Outgoing::Text("1234".into())).unwrap();
        assert_eq!(budget.snapshot()["queue_bytes"], 6);
        assert!(sender.try_send(Outgoing::Text("x".into())).is_err());
        drop(receiver);
        assert_eq!(budget.snapshot()["queue_bytes"], 0);
        assert_eq!(sender.queued(), (0, 0));
    }

    #[tokio::test]
    async fn shared_fanout_keeps_one_payload_and_reserves_each_queue() {
        let payload = Outgoing::shared_text("payload".repeat(64));
        let Outgoing::SharedText(text) = &payload else {
            unreachable!("shared_text constructs SharedText");
        };
        let budget = crate::server::socket_budget::SocketBudget::new(
            crate::server::socket_budget::SocketPolicy {
                queue_bytes_max: 8 * payload.bytes(),
                ..Default::default()
            },
        );
        let mut queues = Vec::new();
        for _ in 0..8 {
            let admit = budget.clone();
            let release = budget.clone();
            let (sender, receiver) = Sender::channel(
                2,
                1024,
                Some(Arc::new(move |frames, bytes| {
                    assert!(frames < 0);
                    release.queue_remove((-frames) as usize, bytes);
                })),
                Some(Arc::new(move |bytes| admit.queue_admit(bytes))),
            );
            sender.try_send(payload.clone()).unwrap();
            assert_eq!(sender.queued(), (1, text.len() + 2));
            queues.push((sender, receiver));
        }
        assert_eq!(budget.snapshot()["queue_bytes"], 8 * payload.bytes());
        // Sharing storage must not let a recipient bypass deployment limits.
        assert!(queues[0].0.try_send(payload.clone()).is_err());

        for (sender, mut receiver) in queues {
            let queued = receiver.recv().await.unwrap();
            let (outgoing, reservation) = queued.into_parts();
            let Outgoing::SharedText(received) = outgoing else {
                unreachable!("production fanout keeps SharedText");
            };
            let original = axum::body::Bytes::from(text.clone());
            let received = axum::body::Bytes::from(received);
            assert_eq!(original.as_ptr(), received.as_ptr());
            assert_eq!(sender.queued(), (1, payload.bytes()));
            drop(reservation);
            assert_eq!(sender.queued(), (0, 0));
        }
        assert_eq!(budget.snapshot()["queue_bytes"], 0);
    }

    /// A repeatable smoke benchmark for the complete production path. It
    /// compares per-recipient String allocation with one owned `Utf8Bytes`
    /// allocation cloned across all recipients, for both a small and a large
    /// frame. Run with `cargo test -p librepaper --lib
    /// room::outgoing::budget_tests::benchmark_shared_fanout -- --ignored
    /// --nocapture`.
    #[test]
    #[ignore]
    fn benchmark_shared_fanout() {
        const RECIPIENTS: usize = 64;
        const ROUNDS: usize = 1_000;
        const TRIALS: usize = 3;

        fn run(payload_len: usize, shared: bool) -> std::time::Duration {
            let source = "x".repeat(payload_len);
            let mut queues = (0..RECIPIENTS)
                .map(|_| Sender::channel(2, payload_len + 1024, None, None))
                .collect::<Vec<_>>();
            let started = std::time::Instant::now();
            for _ in 0..ROUNDS {
                let shared_payload = shared.then(|| Outgoing::shared_text(source.clone()));
                for (sender, _) in &queues {
                    let outgoing = match &shared_payload {
                        Some(payload) => payload.clone(),
                        None => Outgoing::Text(source.clone()),
                    };
                    sender
                        .try_send(outgoing)
                        .expect("fanout recipient queue has capacity");
                }
                for (_, receiver) in &mut queues {
                    let queued = receiver.try_recv().expect("fanout frame is queued");
                    std::hint::black_box(queued.into_parts().0);
                }
            }
            started.elapsed()
        }

        for payload_len in [128, 4096] {
            let mut shared_total = std::time::Duration::ZERO;
            let mut string_total = std::time::Duration::ZERO;
            for trial in 0..TRIALS {
                if trial % 2 == 0 {
                    string_total += run(payload_len, false);
                    shared_total += run(payload_len, true);
                } else {
                    shared_total += run(payload_len, true);
                    string_total += run(payload_len, false);
                }
            }
            let frames = RECIPIENTS * ROUNDS * TRIALS;
            eprintln!(
                "fanout {payload_len}B: String {:?}, SharedText {:?} ({frames} frames each)",
                string_total / TRIALS as u32,
                shared_total / TRIALS as u32,
            );
        }
    }
}
