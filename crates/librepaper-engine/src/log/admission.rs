//! How many decoded documents this process will work on at once (§9.3).
//!
//! The memory budget in [`super::budget`] bounds how much a document may
//! cost; this bounds how many may be paid for at the same time. They are
//! different limits and neither implies the other: a deployment with room
//! for a hundred small documents can still be brought down by a hundred
//! simultaneous Loro imports, because every one of them is synchronous CPU
//! work that cannot be interrupted once it has started.
//!
//! §9.3 names the number: "a dedicated blocking thread pool of
//! `min(cores, 4)` threads behind a semaphore". The pool itself is Tokio's
//! blocking pool, which is sized for blocking I/O and so is far too
//! permissive on its own (its default ceiling is 512 threads); the semaphore
//! here is what actually makes it dedicated. Four is the ceiling rather than
//! the count because the deployments this targets are small VPSs, where
//! leaving cores for the relay path matters more than finishing one build
//! sooner.
//!
//! One permit covers one build or one compaction export, held across the
//! `spawn_blocking` call and released when it returns. Holding a permit
//! never involves waiting for anything else -- no lock, no database, no
//! other permit -- so there is nothing here for two holders to deadlock
//! over.

use std::sync::{Arc, OnceLock};

use tokio::sync::{OwnedSemaphorePermit, Semaphore};

/// The ceiling §9.3 sets, whatever the machine turns out to have.
const MOST: usize = 4;

/// How many of these this process runs at once. Exposed so a caller, or a
/// test, can state the bound rather than hard-coding a number the machine
/// might not have.
pub fn concurrency() -> usize {
    std::thread::available_parallelism()
        .map_or(1, |cores| cores.get())
        .min(MOST)
}

fn semaphore() -> &'static Arc<Semaphore> {
    static HEAVY: OnceLock<Arc<Semaphore>> = OnceLock::new();
    // Not `available_permits()`: that reports what is left, which is the
    // opposite of the bound once anything is running.
    HEAVY.get_or_init(|| Arc::new(Semaphore::new(concurrency())))
}

/// Waits for a turn at uninterruptible Loro work. The permit is released
/// when the returned value drops, including when the caller is cancelled,
/// so an abandoned build does not hold a slot.
pub async fn heavy() -> OwnedSemaphorePermit {
    semaphore()
        .clone()
        .acquire_owned()
        .await
        .expect("the admission semaphore is never closed")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn the_bound_is_never_wider_than_the_spec_allows() {
        assert!(concurrency() >= 1);
        assert!(concurrency() <= MOST);
    }

    /// The point of the semaphore is that the (n+1)th caller waits. Taking
    /// every permit and then asking for one more is the only way to see
    /// that, since the count depends on the machine.
    #[tokio::test]
    async fn one_caller_past_the_bound_waits_until_a_permit_comes_back() {
        let held: Vec<_> = {
            let mut held = Vec::new();
            for _ in 0..concurrency() {
                held.push(heavy().await);
            }
            held
        };
        assert!(
            tokio::time::timeout(Duration::from_millis(20), heavy())
                .await
                .is_err(),
            "a caller past the bound got a permit anyway"
        );
        drop(held);
        assert!(
            tokio::time::timeout(Duration::from_millis(200), heavy())
                .await
                .is_ok(),
            "a returned permit did not reach the waiter"
        );
    }
}
