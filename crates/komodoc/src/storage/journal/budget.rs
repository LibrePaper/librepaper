//! `M`: the memory admission for the transient copies persistence makes.
//!
//! Storage quota bounds the bill; nothing in it bounds what a process holds
//! in RAM while it is saving. One append of a maximum snapshot builds the
//! snapshot, clones it for the journal, fragments it into records, flattens
//! those fragments back for the idempotence check, frames each segment, and
//! clones each framed segment for the object store -- and a compaction adds
//! the recovery base beside all of it. Nothing counted those copies, so a
//! handful of concurrent large rooms could hold hundreds of megabytes with no
//! ceiling anywhere.
//!
//! This is deliberately a byte budget rather than a permit count: the cost of
//! an operation is its snapshot size, and a count would charge a keystroke
//! what it charges a boundary snapshot.

use super::*;

/// A bounded byte budget shared by every room persisting through one journal.
#[derive(Debug)]
pub struct MemoryBudget {
    capacity: usize,
    state: std::sync::Mutex<BudgetState>,
    released: Notify,
}

#[derive(Debug, Default)]
struct BudgetState {
    held: usize,
    /// Bytes belonging to producers already waiting. A task waiting with a
    /// full snapshot in hand is not free, so waiters are bounded too: past
    /// this the next producer is refused now rather than parked holding a
    /// copy of its own.
    waiting: usize,
    peak: usize,
    waiters: usize,
}

/// Ownership of part of the budget. Dropping it is the release, so a
/// cancelled append gives its admission back exactly once and without any
/// await point in the unwind.
#[derive(Debug)]
pub struct MemoryPermit {
    budget: Arc<MemoryBudget>,
    bytes: usize,
}

impl MemoryPermit {
    pub fn bytes(&self) -> usize {
        self.bytes
    }
}

impl Drop for MemoryPermit {
    fn drop(&mut self) {
        {
            let mut state = self.budget.state.lock().expect("memory budget is poisoned");
            state.held = state.held.saturating_sub(self.bytes);
        }
        self.budget.released.notify_waiters();
    }
}

/// One producer's place in the waiting set, released on drop so that a
/// cancelled acquire does not permanently consume the waiting allowance.
struct WaitTicket {
    budget: Arc<MemoryBudget>,
    bytes: usize,
}

impl Drop for WaitTicket {
    fn drop(&mut self) {
        let mut state = self.budget.state.lock().expect("memory budget is poisoned");
        state.waiting = state.waiting.saturating_sub(self.bytes);
        state.waiters = state.waiters.saturating_sub(1);
    }
}

impl MemoryBudget {
    pub fn new(capacity: usize) -> Arc<Self> {
        Arc::new(Self {
            capacity,
            state: std::sync::Mutex::new(BudgetState::default()),
            released: Notify::new(),
        })
    }

    pub fn capacity(&self) -> usize {
        self.capacity
    }

    /// Bytes currently owned by permits. A test counter, and what the
    /// admission decision reads.
    pub fn held_bytes(&self) -> usize {
        self.state.lock().expect("memory budget is poisoned").held
    }

    /// The largest simultaneous holding since this budget was created. The
    /// measurement the size-limit spec asks for is taken from here rather
    /// than from a process-memory sample.
    pub fn peak_bytes(&self) -> usize {
        self.state.lock().expect("memory budget is poisoned").peak
    }

    pub fn waiters(&self) -> usize {
        self.state
            .lock()
            .expect("memory budget is poisoned")
            .waiters
    }

    /// Admit `bytes`, waiting for room if the budget is merely busy.
    ///
    /// A request larger than the whole budget is permanent: no amount of
    /// waiting makes it fit, and parking it would deadlock every other
    /// producer behind work that can never run. A request that would push the
    /// waiting set past the budget is temporary: the caller is told to retry
    /// rather than parked with a snapshot in hand.
    pub async fn acquire(self: &Arc<Self>, bytes: usize) -> JournalResult<MemoryPermit> {
        if bytes > self.capacity {
            return Err(JournalError::Limit(format!(
                "a save of {bytes} bytes is past the {} byte persistence memory budget",
                self.capacity
            )));
        }
        loop {
            // Enrolled before the budget is inspected, because
            // `notify_waiters` stores no permit: a release that lands between
            // the check and the await would otherwise be missed and this
            // producer would sleep with the budget free.
            let notified = self.released.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            {
                let mut state = self.state.lock().expect("memory budget is poisoned");
                if state.held.saturating_add(bytes) <= self.capacity {
                    state.held += bytes;
                    state.peak = state.peak.max(state.held);
                    return Ok(MemoryPermit {
                        budget: Arc::clone(self),
                        bytes,
                    });
                }
                if state.waiting.saturating_add(bytes) > self.capacity {
                    return Err(JournalError::Busy(
                        "too many saves are already waiting for memory".into(),
                    ));
                }
                state.waiting += bytes;
                state.waiters += 1;
            }
            // The registration is owned, so a producer whose future is
            // dropped while parked stops counting against the waiting bound
            // instead of shutting later producers out forever.
            let _ticket = WaitTicket {
                budget: Arc::clone(self),
                bytes,
            };
            notified.await;
        }
    }
}
