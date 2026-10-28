//! One pool of bytes that callers take from and give back to.
//!
//! [`super::budget`] and [`super::pending`] both account bytes against a
//! limit and both let a caller wait for a release. This is the one place that
//! does the compare-and-swap and the wait, so the "register for the wakeup
//! before checking capacity" step is written once.
//!
//! The pool hands out no guard. A caller wraps a successful take in its own
//! reservation type and calls [`Pool::give_back`] from that type's `Drop`.

use std::sync::atomic::{AtomicU64, Ordering};
use tokio::sync::Notify;
use tokio::time::Instant;

/// How long [`Pool::take`] may wait for a release.
#[derive(Clone, Copy, Debug)]
pub enum Wait {
    /// Give up at the deadline.
    Until(Instant),
    /// Wait for as long as it takes. Only for a caller that must not fail
    /// for want of room, such as a mandatory drain.
    Forever,
}

/// The pool had no room: the request is larger than the whole pool, or the
/// deadline passed first.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Refused;

pub struct Pool {
    limit: u64,
    used: AtomicU64,
    /// Woken whenever bytes are given back, so a waiter retries at once
    /// rather than on a timer.
    released: Notify,
}

impl Pool {
    pub fn new(limit: u64) -> Self {
        Self {
            limit,
            used: AtomicU64::new(0),
            released: Notify::new(),
        }
    }

    pub fn limit(&self) -> u64 {
        self.limit
    }

    pub fn used(&self) -> u64 {
        self.used.load(Ordering::Acquire)
    }

    /// Takes `bytes` if they are there and never partially. Checked
    /// arithmetic: a size that overflows is refused rather than wrapped into
    /// a small number that would be admitted.
    pub fn try_take(&self, bytes: u64) -> bool {
        let mut current = self.used.load(Ordering::Acquire);
        loop {
            let Some(wanted) = current.checked_add(bytes) else {
                return false;
            };
            if wanted > self.limit {
                return false;
            }
            match self.used.compare_exchange_weak(
                current,
                wanted,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => return true,
                Err(seen) => current = seen,
            }
        }
    }

    /// Returns bytes taken earlier and wakes every waiter, each of which
    /// rechecks whether it now fits.
    pub fn give_back(&self, bytes: u64) {
        self.used.fetch_sub(bytes, Ordering::AcqRel);
        self.released.notify_waiters();
    }

    /// Takes `bytes`, waiting for releases as `wait` allows. A request larger
    /// than the whole pool is refused at once, because no release could ever
    /// make room for it.
    pub async fn take(&self, bytes: u64, wait: Wait) -> Result<(), Refused> {
        if bytes > self.limit {
            return Err(Refused);
        }
        loop {
            let released = self.released.notified();
            tokio::pin!(released);
            // Register before checking, so a release between the failed
            // attempt and the wait is not missed.
            released.as_mut().enable();
            if self.try_take(bytes) {
                return Ok(());
            }
            match wait {
                Wait::Forever => released.await,
                Wait::Until(deadline) => {
                    if tokio::time::timeout_at(deadline, released).await.is_err() {
                        return Err(Refused);
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::time::Duration;

    #[tokio::test]
    async fn a_take_within_capacity_succeeds_at_once() {
        let pool = Pool::new(100);
        assert_eq!(pool.take(60, Wait::Forever).await, Ok(()));
        assert_eq!(pool.used(), 60);
        let soon = Instant::now() + Duration::from_secs(1);
        assert_eq!(pool.take(40, Wait::Until(soon)).await, Ok(()));
        assert_eq!(pool.used(), 100);
    }

    #[tokio::test]
    async fn a_request_larger_than_the_pool_is_refused_without_waiting() {
        let pool = Pool::new(100);
        assert_eq!(pool.take(101, Wait::Forever).await, Err(Refused));
        assert_eq!(pool.used(), 0);
    }

    #[tokio::test(start_paused = true)]
    async fn a_take_beyond_capacity_waits_for_a_release() {
        let pool = Arc::new(Pool::new(100));
        assert!(pool.try_take(100));
        let waiter = {
            let pool = pool.clone();
            tokio::spawn(async move { pool.take(50, Wait::Forever).await })
        };
        tokio::time::sleep(Duration::from_secs(10)).await;
        assert!(!waiter.is_finished(), "nothing was released yet");
        pool.give_back(100);
        assert_eq!(waiter.await.expect("the waiter finishes"), Ok(()));
        assert_eq!(pool.used(), 50);
    }

    #[tokio::test(start_paused = true)]
    async fn an_expired_deadline_refuses() {
        let pool = Pool::new(100);
        assert!(pool.try_take(100));
        let deadline = Instant::now() + Duration::from_millis(20);
        assert_eq!(pool.take(50, Wait::Until(deadline)).await, Err(Refused));
        assert_eq!(pool.used(), 100);
    }

    #[tokio::test(start_paused = true)]
    async fn forever_is_still_waiting_long_after_any_deadline() {
        let pool = Arc::new(Pool::new(100));
        assert!(pool.try_take(100));
        let waiter = {
            let pool = pool.clone();
            tokio::spawn(async move { pool.take(100, Wait::Forever).await })
        };
        tokio::time::sleep(Duration::from_secs(86_400)).await;
        assert!(!waiter.is_finished());
        pool.give_back(100);
        assert_eq!(waiter.await.expect("the waiter finishes"), Ok(()));
    }

    #[tokio::test(start_paused = true)]
    async fn a_release_satisfies_only_the_waiter_that_now_fits() {
        let pool = Arc::new(Pool::new(100));
        assert!(pool.try_take(100));
        let big = {
            let pool = pool.clone();
            tokio::spawn(async move { pool.take(80, Wait::Forever).await })
        };
        let small = {
            let pool = pool.clone();
            tokio::spawn(async move { pool.take(20, Wait::Forever).await })
        };
        tokio::time::sleep(Duration::from_millis(1)).await;
        pool.give_back(30);
        assert_eq!(small.await.expect("the small waiter"), Ok(()));
        assert!(!big.is_finished(), "30 free never fit 80");
        assert_eq!(pool.used(), 90);
        pool.give_back(90);
        assert_eq!(big.await.expect("the big waiter"), Ok(()));
        assert_eq!(pool.used(), 80);
    }

    /// The release lands after the failed attempt and before the wait. The
    /// waiter must see it through the registered notification, not sleep.
    #[tokio::test(start_paused = true)]
    async fn a_release_between_the_attempt_and_the_wait_is_not_lost() {
        let pool = Arc::new(Pool::new(100));
        assert!(pool.try_take(100));
        let mut take = Box::pin(pool.take(100, Wait::Forever));
        // Poll once: the attempt fails and the future parks on the notify.
        assert!(futures_util::poll!(take.as_mut()).is_pending());
        pool.give_back(100);
        assert_eq!(take.await, Ok(()));
        assert_eq!(pool.used(), 100);
    }
}
