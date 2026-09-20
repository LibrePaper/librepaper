//! One memory budget for everything that holds a decoded document.
//!
//! Encoded bytes bound ingress and storage; they do not bound decoded memory
//! or CPU (SPEC-server-is-a-log §9). Synchronous Loro work cannot be
//! interrupted once started, so the bound is on admission: a build, a fork or
//! a projection reserves its estimate before it begins and releases it when
//! it finishes, and a request that cannot reserve waits briefly and then
//! fails with `busy`.
//!
//! The estimate is base bytes plus row bytes times an expansion factor. The
//! factor is a measurement, not a constant of nature: it is configurable so a
//! deployment can correct it, and the spike in §14.1 is what sets the
//! default.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, OnceLock, Weak};
use std::time::Duration;

use futures_util::future::BoxFuture;
use tokio::sync::Notify;

/// How much larger a decoded document is than the bytes it was loaded from.
///
/// §14.1 says the spike sets this. The spike has now been run
/// (`storage::postgres::benchmarks::typing_throughput_release_benchmark`,
/// twenty resident 1 MiB documents, real RSS, release build) and it
/// measured two figures rather than one, because a document's decoded cost
/// tracks its OPERATION COUNT as much as its byte count and the byte count
/// is all the estimate has to go on. Across a full ten-minute run and two
/// shorter ones:
///
/// | shape | factor |
/// |---|---|
/// | arrived as one upload | 1.66 to 2.34 |
/// | same bytes, 2000 accumulated edits | 3.24 to 3.43 |
///
/// The upload figure is the noisy one, which is expected: it is the smaller
/// delta of the two and so the one page-level allocator behaviour moves
/// most. The typed figure, which is the one that matters because it is
/// worse, held within six per cent across runs.
///
/// Six is kept. It is no longer a guess: it covers the worse measured shape
/// with room for the one thing the spike could not bound, which is a
/// document whose op history is far longer than two thousand edits. The
/// asymmetry is the reason to keep the margin -- reserving too much only
/// makes a read wait or answer `busy`, while reserving too little puts the
/// process out of memory, and §9.2 has no way to recover from the second.
///
/// Lowering it is a real option once somebody measures a long-lived
/// document: at 4 a 512 MiB deployment holds about half again as many
/// documents resident. Re-run the benchmark before changing the number,
/// and read `expansion_measurement` in its report rather than the single
/// `measured_expansion_factor`, which averages the two shapes.
pub const DEFAULT_EXPANSION: u64 = 6;

/// What a cold build costs in memory while it is running, as a multiple of
/// log bytes, reserved for the duration of the import and then released.
///
/// `DEFAULT_EXPANSION` is what a decoded document costs once it sits in the
/// cache. It is not what producing that document costs while the import is
/// in flight: `Sequencer::build` (sequencer.rs) imports row by row into a
/// fresh `LoroDoc`, and that process peaks before it settles there.
///
/// Measured 2026-09-20, release build, as peak RSS during a cold build
/// minus RSS before it began. That is the WHOLE cost of the build,
/// residency included, not the part standing above residency: the process
/// does not offer a way to separate the two, because the document being
/// built is what most of the peak is. Reserving this ON TOP OF the resident
/// estimate therefore over-reserves, by roughly the resident figure itself.
/// That is the direction to be wrong in, and it is only true while the
/// build runs.
///
/// | log size | measured peak, above pre-build RSS |
/// |---|---|
/// | 1 MiB | 3.2 MB |
/// | 2 MiB | 16 MB |
/// | 3.5 MiB | 27 MB |
///
/// The 1 MiB figure is the noisy one (small absolute numbers move a lot
/// relatively); the two larger ones agree at roughly 7 to 8 times log
/// bytes. Eight is kept, for the same reason `DEFAULT_EXPANSION` rounds up
/// rather than down: reserving too much only makes a build wait or answer
/// `busy`, reserving too little runs the process out of memory during
/// exactly the operation that cannot be interrupted (§9.3).
///
/// `Sequencer::build` reserves this on top of the resident estimate for the
/// duration of the import only, as a second reservation released the
/// moment the entry is cached -- the resident estimate is what the cache
/// keeps paying for afterwards, and the two must not be summed into one
/// long-lived reservation or every cached document would overpay for a cost
/// it no longer carries.
pub const BUILD_TRANSIENT_EXPANSION: u64 = 8;

/// What one document is expected to cost resident, from what its log weighs.
pub fn estimate(encoded_bytes: u64, expansion: u64) -> u64 {
    // A floor, because a document of three keystrokes still costs a Loro
    // document's fixed structures, and a reservation of nearly nothing would
    // let an unbounded number of them in.
    const FLOOR: u64 = 64 * 1024;
    encoded_bytes.saturating_mul(expansion).max(FLOOR)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Busy;

impl std::fmt::Display for Busy {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("this deployment has no memory to read that document right now")
    }
}

impl std::error::Error for Busy {}

/// Who to ask to give memory back when there is none left.
///
/// §9.2 says a reservation that cannot be met evicts before it fails:
/// "eviction removes cold entries first and may remove entries with
/// subscribers; under the opaque log nothing depends on an entry staying
/// resident." Without this the budget could only ever wait and then answer
/// `busy`, which is what it did until somebody noticed that
/// `Registry::evict_caches` had no caller at all -- the mechanism existed
/// and nothing reached it, so memory pressure refused reads instead of
/// making room for them.
pub trait Evict: Send + Sync {
    /// Free at least `wanted` bytes if you can, and answer with how much
    /// you actually freed.
    fn evict(&self, wanted: u64) -> BoxFuture<'_, u64>;
}

/// The process-wide budget.
pub struct Budget {
    limit: u64,
    used: AtomicU64,
    expansion: u64,
    /// Woken whenever a reservation is released, so a waiter retries at once
    /// rather than on a timer.
    released: Notify,
    /// The registry, weakly, so the budget it owns does not keep it alive.
    /// Set once, just after both exist.
    evictor: OnceLock<Weak<dyn Evict>>,
    /// Cache entries dropped to make room, cumulative. A deployment that is
    /// thrashing -- every read a rebuild because entries keep evicting one
    /// another -- is indistinguishable from a healthy one without this and
    /// the counter below (§9.2).
    evicted: AtomicU64,
    /// Reservations that were refused for want of memory, cumulative. Every
    /// path that gives up on memory counts here, patient or not: a counter
    /// that sees one of two refusal paths is worse than no counter, because
    /// a zero then means nothing.
    refused: AtomicU64,
}

impl Budget {
    pub fn new(limit_bytes: u64, expansion: u64) -> Arc<Self> {
        Arc::new(Self {
            limit: limit_bytes.max(1),
            used: AtomicU64::new(0),
            expansion: expansion.max(1),
            released: Notify::new(),
            evictor: OnceLock::new(),
            evicted: AtomicU64::new(0),
            refused: AtomicU64::new(0),
        })
    }

    /// Names who to ask for memory back. Called once by the registry, which
    /// cannot be passed to `new` because the budget is built first.
    pub fn evicts_through(&self, evictor: Weak<dyn Evict>) {
        let _ = self.evictor.set(evictor);
    }

    pub fn expansion(&self) -> u64 {
        self.expansion
    }

    pub fn limit(&self) -> u64 {
        self.limit
    }

    pub fn used(&self) -> u64 {
        self.used.load(Ordering::Acquire)
    }

    pub fn evicted(&self) -> u64 {
        self.evicted.load(Ordering::Relaxed)
    }

    pub fn refused(&self) -> u64 {
        self.refused.load(Ordering::Relaxed)
    }

    /// Counts entries the registry dropped to make room. Called by the
    /// evictor, because only it knows how many it actually let go of.
    pub fn note_evicted(&self, entries: u64) {
        self.evicted.fetch_add(entries, Ordering::Relaxed);
    }

    /// The whole snapshot an operator reads (`GET /api/status`). Read on
    /// demand: nothing here is sampled or aggregated on a timer.
    pub fn snapshot(&self) -> serde_json::Value {
        serde_json::json!({
            "limit_bytes": self.limit,
            "reserved_bytes": self.used(),
            "expansion": self.expansion,
            "evicted_caches": self.evicted(),
            "refused_busy": self.refused(),
        })
    }

    fn refuse(&self) -> Busy {
        self.refused.fetch_add(1, Ordering::Relaxed);
        Busy
    }

    /// Takes `bytes` if they are there. Never partial: a build that cannot
    /// have its whole estimate must not start, because it cannot be stopped
    /// half way.
    pub fn try_reserve(self: &Arc<Self>, bytes: u64) -> Option<Reservation> {
        let mut current = self.used.load(Ordering::Acquire);
        loop {
            let wanted = current.saturating_add(bytes);
            if wanted > self.limit {
                return None;
            }
            match self.used.compare_exchange_weak(
                current,
                wanted,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => {
                    return Some(Reservation {
                        budget: self.clone(),
                        bytes,
                    })
                }
                Err(seen) => current = seen,
            }
        }
    }

    /// Takes `bytes` or refuses at once, for a caller that cannot await.
    /// Unlike [`Self::try_reserve`], which [`Self::reserve`] also uses while
    /// it is still waiting, a failure here is a refusal and is counted.
    pub fn try_reserve_now(self: &Arc<Self>, bytes: u64) -> Result<Reservation, Busy> {
        self.try_reserve(bytes).ok_or_else(|| self.refuse())
    }

    /// Waits up to `patience` for room, then gives up. Callers that can evict
    /// something do so before asking again; callers that cannot answer
    /// `busy`.
    pub async fn reserve(
        self: &Arc<Self>,
        bytes: u64,
        patience: Duration,
    ) -> Result<Reservation, Busy> {
        if bytes > self.limit {
            return Err(self.refuse());
        }
        let deadline = tokio::time::Instant::now() + patience;
        // One eviction pass, before waiting rather than after: a caller that
        // waits out its whole patience and only then makes room has spent
        // the patience for nothing, and the entries it would have evicted
        // were cold the entire time.
        if self.try_reserve(bytes).is_none() {
            if let Some(evictor) = self.evictor.get().and_then(Weak::upgrade) {
                evictor.evict(bytes).await;
            }
        }
        loop {
            if let Some(reservation) = self.try_reserve(bytes) {
                return Ok(reservation);
            }
            let waiting = self.released.notified();
            // Re-check after arming the notification, so a release between
            // the failed attempt above and the wait below is not missed.
            if let Some(reservation) = self.try_reserve(bytes) {
                return Ok(reservation);
            }
            if tokio::time::timeout_at(deadline, waiting).await.is_err() {
                return Err(self.refuse());
            }
        }
    }
}

/// Held for as long as the memory is. Releasing is `Drop`, so a cancelled
/// future gives its reservation back rather than leaking it -- which is the
/// one failure mode that turns a busy minute into a permanently unusable
/// deployment.
pub struct Reservation {
    budget: Arc<Budget>,
    bytes: u64,
}

impl Reservation {
    pub fn bytes(&self) -> u64 {
        self.bytes
    }

    /// Extends an existing reservation atomically. The original reservation
    /// remains unchanged when the budget cannot cover the requested size.
    pub fn try_grow_to(&mut self, bytes: u64) -> Result<(), Busy> {
        if bytes <= self.bytes {
            return Ok(());
        }
        let mut growth = self.budget.clone().try_reserve_now(bytes - self.bytes)?;
        self.bytes = bytes;
        // Transfer the newly charged bytes into `self`; `growth` must not
        // return them when its temporary guard drops.
        growth.bytes = 0;
        Ok(())
    }
}

impl Drop for Reservation {
    fn drop(&mut self) {
        self.budget.used.fetch_sub(self.bytes, Ordering::AcqRel);
        self.budget.released.notify_waiters();
    }
}

impl std::fmt::Debug for Reservation {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "Reservation({} bytes)", self.bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_reservation_is_given_back_when_it_is_dropped() {
        let budget = Budget::new(100, 1);
        let first = budget.try_reserve(60).expect("room for the first");
        assert!(budget.try_reserve(60).is_none(), "no room for a second");
        drop(first);
        assert!(budget.try_reserve(60).is_some(), "the first gave it back");
    }

    #[test]
    fn a_reservation_larger_than_the_budget_is_never_granted() {
        let budget = Budget::new(100, 1);
        assert!(budget.try_reserve(101).is_none());
    }

    #[tokio::test]
    async fn waiting_gives_up_rather_than_hanging() {
        let budget = Budget::new(100, 1);
        let _held = budget.try_reserve(100).expect("room");
        assert_eq!(
            budget.reserve(50, Duration::from_millis(20)).await.err(),
            Some(Busy)
        );
    }

    #[tokio::test]
    async fn a_waiter_is_woken_by_a_release() {
        let budget = Budget::new(100, 1);
        let held = budget.try_reserve(100).expect("room");
        let waiting = {
            let budget = budget.clone();
            tokio::spawn(async move { budget.reserve(100, Duration::from_secs(5)).await.is_ok() })
        };
        tokio::task::yield_now().await;
        drop(held);
        assert!(waiting.await.expect("the waiter finishes"));
    }

    /// A refusal counter an operator can read has to see every refusal, or a
    /// zero says "healthy" when the deployment is turning reads away.
    #[tokio::test]
    async fn every_way_of_refusing_for_memory_is_counted_once() {
        let budget = Budget::new(100, 1);
        let _held = budget.try_reserve(100).expect("room");
        assert!(budget.try_reserve(50).is_none());
        assert_eq!(
            budget.refused(),
            0,
            "a bare try is what `reserve` retries with; it has refused nobody",
        );
        assert!(budget.try_reserve_now(50).is_err(), "no room, no waiting");
        assert!(budget.reserve(50, Duration::from_millis(20)).await.is_err());
        assert!(
            budget
                .reserve(101, Duration::from_millis(20))
                .await
                .is_err(),
            "larger than the whole budget, refused without waiting",
        );
        assert_eq!(budget.refused(), 3);
    }

    #[test]
    fn the_estimate_has_a_floor() {
        assert_eq!(estimate(0, 6), 64 * 1024);
        assert_eq!(estimate(1024 * 1024, 6), 6 * 1024 * 1024);
    }
}
