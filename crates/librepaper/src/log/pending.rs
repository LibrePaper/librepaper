//! One deployment-wide bound on unsaved source bytes, and on the temporary
//! allocations needed to save them.
//!
//! [`super::budget`] bounds DECODED documents: caches, forks, builds. That
//! memory is a cache and can be evicted, which is what makes a patient
//! reservation and an eviction pass the right shape for it. Pending source
//! bytes are the opposite of a cache: they are somebody's typing that has not
//! reached PostgreSQL yet, nothing may drop them to make room, and the only
//! way they leave memory is by being written. So they get their own
//! accounting rather than a second meaning for the decoded budget.
//!
//! ## Two pools, and why
//!
//! `retained` is what the document buffers hold. `scratch` is what writing
//! them costs while the write is in flight: the encoded row and both driver
//! buffers, including capacity growth.
//!
//! They are separate because the one thing this must never do is let unsaved
//! work fill the whole allowance and leave no room to save any of it. A
//! single undifferentiated pool has exactly that failure: every byte admitted
//! into a buffer is a byte the flush that would drain it cannot have, and a
//! deployment at its limit would be permanently unable to make progress --
//! which is worse than the unbounded growth this replaces, because unbounded
//! growth at least recovers when the database does.
//!
//! With two pools the argument is short. `ingest` spends only `retained`;
//! [`Sequencer::flush`](super::Sequencer::flush) and the semantic-command
//! path spend only `scratch`. A full `retained` pool therefore cannot take a
//! byte of `scratch`, the configuration is refused unless `scratch` can hold
//! one maximum-size row (see `Configuration::validate_pending`), and every
//! scratch holder is an in-flight persistence future that finishes or is
//! cancelled and releases by `Drop` either way. So there is always, at the
//! limit, room for at least one document to drain.
//!
//! ## What this does NOT bound
//!
//! It is not a process-RSS limit and must not be read as one. Decoded CRDT
//! memory is [`super::budget`]'s; outbound socket queues are
//! `server::socket_budget`'s; inbound request bodies are
//! `cost.request_body_memory_bytes`'; one assembling socket update is
//! `socket.rs`' `update_ceiling`. Allocator fragmentation and the runtime's
//! own overhead are nobody's.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use tokio::sync::Notify;

/// What a deployment-wide pending refusal is. Always retryable: the bytes
/// standing in the way are somebody else's unsaved work, and it leaves as it
/// is written.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Pressure;

impl std::fmt::Display for Pressure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("this deployment is holding all the unsaved work it can")
    }
}

impl std::error::Error for Pressure {}

/// Which pool a reservation came from. Only used to keep the two `Drop`
/// implementations from being written twice.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Pool {
    Retained,
    Scratch,
}

/// The one accounting authority, built by [`super::Registry`] and handed to
/// every sequencer it admits.
pub struct PendingBudget {
    retained_limit: u64,
    retained_used: AtomicU64,
    scratch_limit: u64,
    scratch_used: AtomicU64,
    released: Notify,
    /// Updates refused for want of retained capacity, cumulative. The number
    /// an operator reads to tell a deployment that is merely busy from one
    /// that is turning typing away.
    refused_retained: AtomicU64,
    /// Persistence attempts that could not take scratch, cumulative. A
    /// non-zero value here means flushes are being deferred to a later
    /// housekeeping pass, which is survivable but is the thing to watch.
    refused_scratch: AtomicU64,
}

impl PendingBudget {
    pub fn new(retained_limit_bytes: u64, scratch_limit_bytes: u64) -> Arc<Self> {
        Arc::new(Self {
            retained_limit: retained_limit_bytes.max(1),
            retained_used: AtomicU64::new(0),
            scratch_limit: scratch_limit_bytes.max(1),
            scratch_used: AtomicU64::new(0),
            released: Notify::new(),
            refused_retained: AtomicU64::new(0),
            refused_scratch: AtomicU64::new(0),
        })
    }

    pub fn retained_limit(&self) -> u64 {
        self.retained_limit
    }

    pub fn scratch_limit(&self) -> u64 {
        self.scratch_limit
    }

    pub fn retained_used(&self) -> u64 {
        self.retained_used.load(Ordering::Acquire)
    }

    pub fn scratch_used(&self) -> u64 {
        self.scratch_used.load(Ordering::Acquire)
    }

    pub fn refused_retained(&self) -> u64 {
        self.refused_retained.load(Ordering::Relaxed)
    }

    pub fn refused_scratch(&self) -> u64 {
        self.refused_scratch.load(Ordering::Relaxed)
    }

    /// Takes room for a payload that is about to be held in a document
    /// buffer. Non-blocking by design: this runs on the typing path, and a
    /// keystroke must not queue behind a database recovery.
    pub fn try_retain(self: &Arc<Self>, bytes: u64) -> Result<Reservation, Pressure> {
        match take(&self.retained_used, self.retained_limit, bytes) {
            Some(()) => Ok(Reservation {
                budget: self.clone(),
                pool: Pool::Retained,
                bytes,
            }),
            None => {
                self.refused_retained.fetch_add(1, Ordering::Relaxed);
                Err(Pressure)
            }
        }
    }

    /// Takes room for the temporary allocations one persistence attempt
    /// makes. Also non-blocking: a flush that cannot have its scratch gives
    /// up and is asked again by the next housekeeping pass, which is a
    /// deferral rather than a loss -- the buffer and its retained charge are
    /// untouched.
    pub fn try_scratch(self: &Arc<Self>, bytes: u64) -> Result<Reservation, Pressure> {
        match take(&self.scratch_used, self.scratch_limit, bytes) {
            Some(()) => Ok(Reservation {
                budget: self.clone(),
                pool: Pool::Scratch,
                bytes,
            }),
            None => {
                self.refused_scratch.fetch_add(1, Ordering::Relaxed);
                Err(Pressure)
            }
        }
    }

    /// Mandatory drains wait without holding a document lock. Register the
    /// waiter before checking capacity to avoid missing a concurrent release.
    pub async fn reserve_scratch(self: &Arc<Self>, bytes: u64) -> Result<Reservation, Pressure> {
        if bytes > self.scratch_limit {
            return Err(Pressure);
        }
        loop {
            let released = self.released.notified();
            tokio::pin!(released);
            released.as_mut().enable();
            if let Ok(reservation) = self.try_scratch(bytes) {
                return Ok(reservation);
            }
            released.await;
        }
    }

    /// The whole snapshot an operator reads (`GET /api/status`). Read on
    /// demand: nothing here is sampled or aggregated on a timer.
    ///
    /// `reserved` and not `bytes`: the scratch figure is what persistence
    /// ASKED FOR, which deliberately over-counts the payload (see
    /// [`scratch_for`]). Naming it after the payload would invite an operator
    /// to read it as one.
    pub fn snapshot(&self) -> serde_json::Value {
        serde_json::json!({
            "retained_limit_bytes": self.retained_limit,
            "retained_reserved_bytes": self.retained_used(),
            "scratch_limit_bytes": self.scratch_limit,
            "scratch_reserved_bytes": self.scratch_used(),
            "refused_retained": self.refused_retained(),
            "refused_scratch": self.refused_scratch(),
        })
    }
}

/// One compare-and-swap loop, shared by both pools. Checked arithmetic
/// throughout: a size that overflows is refused rather than wrapped into a
/// small number that would be admitted.
fn take(used: &AtomicU64, limit: u64, bytes: u64) -> Option<()> {
    let mut current = used.load(Ordering::Acquire);
    loop {
        let wanted = current.checked_add(bytes)?;
        if wanted > limit {
            return None;
        }
        match used.compare_exchange_weak(current, wanted, Ordering::AcqRel, Ordering::Acquire) {
            Ok(_) => return Some(()),
            Err(seen) => current = seen,
        }
    }
}

/// Held for exactly as long as the allocation it paid for.
///
/// Deliberately not `Clone`: a copy of a payload is a second allocation and
/// needs a second reservation, and a reservation that could be cloned would
/// let one be made by accident. Moving it transfers the charge intact, which
/// is how an accepted payload's charge travels from `ingest` into the buffer.
pub struct Reservation {
    budget: Arc<PendingBudget>,
    pool: Pool,
    bytes: u64,
}

impl Reservation {
    /// Transfer part of a reservation to another allocation owner.
    pub fn split(&mut self, bytes: u64) -> Self {
        assert!(bytes <= self.bytes, "reservation split exceeds its charge");
        self.bytes -= bytes;
        Self {
            budget: self.budget.clone(),
            pool: self.pool,
            bytes,
        }
    }

    pub fn bytes(&self) -> u64 {
        self.bytes
    }
}

impl Drop for Reservation {
    fn drop(&mut self) {
        let counter = match self.pool {
            Pool::Retained => &self.budget.retained_used,
            Pool::Scratch => &self.budget.scratch_used,
        };
        counter.fetch_sub(self.bytes, Ordering::AcqRel);
        if self.pool == Pool::Scratch {
            self.budget.released.notify_waiters();
        }
    }
}

impl std::fmt::Debug for Reservation {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "Reservation({:?}, {} bytes)",
            self.pool, self.bytes
        )
    }
}

/// Room for small per-write allocations that are not the row itself: the
/// encoded version vector, the format and main-path strings stamped on the
/// document row, and the `FlushRow` around them. None of them scales with the
/// buffer; a fixed allowance is honest and keeps the estimate readable.
pub const SCRATCH_SLACK: u64 = 8 * 1024;

/// SQLx 0.8.6 copies the row into PgArguments and again into the
/// connection's Bind write buffer. Each Vec may grow to twice its needed
/// size. The connection guard holds this allowance until those buffers are
/// shrunk on success or destroyed on error/cancellation.
pub fn driver_scratch_for(row_bytes: u64) -> u64 {
    row_bytes.saturating_add(SCRATCH_SLACK).saturating_mul(4)
}

/// One exactly preallocated row plus both driver buffers and their capacity
/// growth. Small per-query fields are included in each driver's allowance.
pub fn scratch_for(row_bytes: u64) -> u64 {
    row_bytes.saturating_add(driver_scratch_for(row_bytes))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_reservation_is_given_back_when_it_is_dropped() {
        let budget = PendingBudget::new(100, 100);
        let held = budget.try_retain(60).expect("room");
        assert_eq!(budget.retained_used(), 60);
        assert!(budget.try_retain(60).is_err(), "no room for a second");
        drop(held);
        assert_eq!(budget.retained_used(), 0);
        assert!(budget.try_retain(60).is_ok(), "the first gave it back");
    }

    /// The whole point of two pools: a full retained pool leaves persistence
    /// its capacity, or nothing that was accepted could ever be written.
    #[test]
    fn a_full_retained_pool_leaves_scratch_untouched() {
        let budget = PendingBudget::new(100, 100);
        let _full = budget.try_retain(100).expect("the whole retained pool");
        assert!(budget.try_retain(1).is_err());
        assert!(
            budget.try_scratch(100).is_ok(),
            "scratch is a separate pool and a full buffer cannot have spent it"
        );
    }

    #[test]
    fn the_two_pools_are_counted_and_refused_separately() {
        let budget = PendingBudget::new(100, 100);
        assert!(budget.try_retain(101).is_err());
        assert!(budget.try_scratch(101).is_err());
        assert_eq!(budget.refused_retained(), 1);
        assert_eq!(budget.refused_scratch(), 1);
        let scratch = budget.try_scratch(40).expect("room");
        assert_eq!(budget.scratch_used(), 40);
        assert_eq!(budget.retained_used(), 0, "a scratch take is not a retain");
        drop(scratch);
        assert_eq!(budget.scratch_used(), 0);
    }

    /// A size that would wrap must refuse rather than be admitted as a small
    /// number, which is the whole reason the counter arithmetic is checked.
    #[test]
    fn an_overflowing_size_refuses_rather_than_wrapping() {
        let budget = PendingBudget::new(u64::MAX, u64::MAX);
        let _held = budget.try_retain(u64::MAX / 2 + 2).expect("room");
        assert!(budget.try_retain(u64::MAX / 2 + 2).is_err());
        assert_eq!(budget.refused_retained(), 1);
    }

    /// Racing takes must not oversubscribe the pool. Every thread asks for
    /// the same size, and exactly as many as the pool holds may win.
    #[test]
    fn concurrent_takes_cannot_oversubscribe_the_pool() {
        let budget = PendingBudget::new(10 * 64, 1);
        let barrier = Arc::new(std::sync::Barrier::new(32));
        let winners = Arc::new(AtomicU64::new(0));
        let mut threads = Vec::new();
        for _ in 0..32 {
            let budget = budget.clone();
            let barrier = barrier.clone();
            let winners = winners.clone();
            threads.push(std::thread::spawn(move || {
                barrier.wait();
                let held = budget.try_retain(64);
                if held.is_ok() {
                    winners.fetch_add(1, Ordering::Relaxed);
                }
                // Held until every thread has finished asking, so a late
                // release cannot make room for a thirty-third winner.
                barrier.wait();
                drop(held);
            }));
        }
        for thread in threads {
            thread.join().expect("a reserving thread");
        }
        assert_eq!(winners.load(Ordering::Relaxed), 10);
        assert_eq!(budget.retained_used(), 0, "every winner gave it back");
    }

    #[test]
    fn scratch_covers_the_row_and_the_drivers_copy_of_it() {
        assert_eq!(scratch_for(1000), 5000 + 4 * SCRATCH_SLACK);
        assert_eq!(scratch_for(0), 4 * SCRATCH_SLACK);
    }
}
