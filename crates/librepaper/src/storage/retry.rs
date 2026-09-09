//! Bounded retries for the object store.
//!
//! A bucket answers "not right now" in several ways -- 429, 503, a 5xx from a
//! gateway, a connection that goes away mid-request -- and none of them mean
//! the caller should give up. Without retries, one throttled request fails a
//! whole checkpoint, a maintenance pass, or a room's lease renewal, and the
//! caller's own recovery is far more expensive than waiting 200 milliseconds.
//!
//! Retrying is only safe when it is bounded. A policy here bounds two things
//! at once: how many attempts an operation may make, and how long it may
//! spend scheduling them. The second bound is the one that matters under a
//! provider-wide outage, where every request fails slowly and an
//! attempt-only bound still lets one operation hold a task for minutes.
//!
//! The clock and the jitter source are injected so a test can assert both
//! bounds exactly, without sleeping and without a timing-only assertion.
//!
//! Nothing here decides *what* is retryable. Failure classification belongs
//! with the protocol that produced it -- see `storage::s3` -- because only
//! the caller knows whether an ambiguous write has been reconciled yet.

use std::time::Duration;

#[cfg(test)]
use std::sync::atomic::{AtomicU64, Ordering};
#[cfg(test)]
use std::sync::Arc;

use async_trait::async_trait;

use crate::storage::blob::{BlobError, BlobResult};

/// What one attempt at a storage operation produced.
pub enum Attempt<T> {
    /// A final answer, success or failure. Report it and stop.
    Settled(BlobResult<T>),
    /// A transient failure whose operation may be attempted again. `after` is
    /// a delay the provider asked for, which is honoured as a floor.
    Transient {
        why: String,
        after: Option<Duration>,
    },
}

/// Where time and randomness come from. Production uses the wall clock and a
/// real random source; a test supplies one that advances a counter instead of
/// waiting, so the bounds below are asserted rather than approximated.
#[async_trait]
pub trait RetryClock: Send + Sync {
    /// Milliseconds since an arbitrary origin. Only differences are used.
    fn now_ms(&self) -> u64;
    /// Waits. This is a plain await point: dropping the future that owns it
    /// is how an operation is cancelled, and no further attempt is made.
    async fn sleep(&self, delay: Duration);
    /// A fraction in `[0, 1)`, used to spread retries of concurrent callers.
    fn jitter(&self) -> f64;
}

/// The wall clock, `tokio::time::sleep`, and the thread random source.
pub struct SystemClock {
    origin: std::time::Instant,
}

impl Default for SystemClock {
    fn default() -> Self {
        SystemClock {
            origin: std::time::Instant::now(),
        }
    }
}

#[async_trait]
impl RetryClock for SystemClock {
    fn now_ms(&self) -> u64 {
        self.origin.elapsed().as_millis() as u64
    }

    async fn sleep(&self, delay: Duration) {
        tokio::time::sleep(delay).await;
    }

    fn jitter(&self) -> f64 {
        rand::random::<f64>()
    }
}

/// How far one operation may go before it reports failure.
///
/// `attempts` counts the first try, so `attempts: 1` disables retrying.
/// `total` bounds the time spent *scheduling* retries: a new attempt is never
/// started once it is spent. It does not shorten a single request that is
/// already in flight -- that is the HTTP client's own timeout, which is
/// deliberately much longer because a large upload is not a stall.
#[derive(Clone, Copy, Debug)]
pub struct RetryLimits {
    pub attempts: u32,
    pub total: Duration,
    pub base: Duration,
    pub cap: Duration,
}

impl RetryLimits {
    /// Requests that can be repeated without changing anything: GET, HEAD,
    /// listing, and DELETE. These get the widest budget because repeating one
    /// costs only bandwidth.
    pub const IDEMPOTENT: RetryLimits = RetryLimits {
        attempts: 4,
        total: Duration::from_secs(30),
        base: Duration::from_millis(100),
        cap: Duration::from_secs(5),
    };

    /// Writes. Every attempt after the first is preceded by reconciling the
    /// object's actual state, so the budget also has to cover those reads;
    /// it is smaller because a caller holding a room's state is waiting on it.
    pub const WRITE: RetryLimits = RetryLimits {
        attempts: 5,
        total: Duration::from_secs(20),
        base: Duration::from_millis(200),
        cap: Duration::from_secs(5),
    };

    /// No retrying at all, for a caller that has its own recovery.
    #[allow(dead_code)]
    pub const ONCE: RetryLimits = RetryLimits {
        attempts: 1,
        total: Duration::ZERO,
        base: Duration::ZERO,
        cap: Duration::ZERO,
    };
}

/// One operation's remaining budget: attempts made so far, and when it began.
///
/// A caller that cannot use `with_retries` -- batch deletion, whose retries
/// carry a shrinking set of keys rather than one result -- drives this
/// directly, so both shapes obey the same bounds.
pub struct RetryWindow<'a> {
    clock: &'a dyn RetryClock,
    limits: RetryLimits,
    started_ms: u64,
    attempts: u32,
}

impl<'a> RetryWindow<'a> {
    pub fn new(clock: &'a dyn RetryClock, limits: RetryLimits) -> RetryWindow<'a> {
        RetryWindow {
            clock,
            limits,
            started_ms: clock.now_ms(),
            attempts: 0,
        }
    }

    /// Claims the next attempt, or returns false when none is left. Call this
    /// before every request; `wait` goes between two of them.
    pub fn begin(&mut self) -> bool {
        if self.attempts >= self.limits.attempts {
            return false;
        }
        self.attempts += 1;
        true
    }

    /// How many attempts have been claimed, for an error message.
    pub fn attempts(&self) -> u32 {
        self.attempts
    }

    /// Waits before the next attempt. Returns false -- and waits for nothing
    /// -- when either bound is spent, which is the caller's signal to report
    /// the failure it is holding.
    ///
    /// `after` is a provider-requested delay: it raises the wait, never
    /// lowers it, and a provider asking for longer than the whole remaining
    /// budget ends the operation rather than being silently ignored.
    pub async fn wait(&mut self, after: Option<Duration>) -> bool {
        if self.attempts >= self.limits.attempts {
            return false;
        }
        let delay = self.backoff(after);
        let elapsed = self.clock.now_ms().saturating_sub(self.started_ms);
        let remaining = self.limits.total.as_millis() as u64;
        if elapsed.saturating_add(delay.as_millis() as u64) > remaining {
            return false;
        }
        self.clock.sleep(delay).await;
        true
    }

    /// Exponential backoff with jitter, floored by a provider's own delay.
    ///
    /// The jitter is applied to the lower half of the interval rather than
    /// the whole of it: two callers throttled at the same moment must not
    /// come back at the same moment, but neither should a single retry wait
    /// arbitrarily close to zero and be throttled again immediately.
    fn backoff(&self, after: Option<Duration>) -> Duration {
        let step = self
            .limits
            .base
            .saturating_mul(1u32 << self.attempts.saturating_sub(1).min(16));
        let step = step.min(self.limits.cap);
        let millis = step.as_millis() as f64;
        let jittered = Duration::from_millis((millis * (0.5 + 0.5 * self.clock.jitter())) as u64);
        match after {
            Some(requested) => jittered.max(requested),
            None => jittered,
        }
    }
}

/// Runs one operation until it settles or its bounds are spent.
///
/// The closure receives the attempt number, one-based, and decides what its
/// own failures mean. Exhaustion is reported as `Other`: a transient failure
/// that never resolved is not a missing object and not a lost conditional
/// write, and turning it into either would let a caller draw a durable
/// conclusion from a temporary outage.
pub async fn with_retries<T, F, Fut>(
    clock: &dyn RetryClock,
    limits: RetryLimits,
    what: &str,
    mut attempt: F,
) -> BlobResult<T>
where
    F: FnMut(u32) -> Fut,
    Fut: std::future::Future<Output = Attempt<T>>,
{
    let mut window = RetryWindow::new(clock, limits);
    let mut last = String::new();
    while window.begin() {
        match attempt(window.attempts()).await {
            Attempt::Settled(result) => return result,
            Attempt::Transient { why, after } => {
                last = why;
                if !window.wait(after).await {
                    break;
                }
            }
        }
    }
    Err(BlobError::Other(format!(
        "{what} did not succeed in {} attempt(s): {last}",
        window.attempts()
    )))
}

/// A clock that never waits, for tests. It advances its own notion of time by
/// exactly the delay asked for, so an assertion about the total bound is an
/// assertion about the policy and not about how busy the machine was.
///
/// `hold` makes `sleep` park instead of returning, which is how the
/// cancellation test gets a window in which to drop the operation.
#[cfg(test)]
pub struct TestClock {
    now: AtomicU64,
    jitter: f64,
    slept: std::sync::Mutex<Vec<Duration>>,
    hold: Option<Arc<Held>>,
}

#[cfg(test)]
pub struct Held {
    /// Released once per parked sleeper, so a test can wait for one
    /// deterministically instead of polling.
    pub parked: tokio::sync::Semaphore,
    pub release: tokio::sync::Notify,
}

#[cfg(test)]
impl TestClock {
    /// A clock whose jitter is fixed, so backoff is exactly reproducible.
    pub fn new(jitter: f64) -> Arc<TestClock> {
        Arc::new(TestClock {
            now: AtomicU64::new(0),
            jitter,
            slept: std::sync::Mutex::new(Vec::new()),
            hold: None,
        })
    }

    /// A clock whose sleeps park until the returned handle releases them.
    pub fn held(jitter: f64) -> (Arc<TestClock>, Arc<Held>) {
        let hold = Arc::new(Held {
            parked: tokio::sync::Semaphore::new(0),
            release: tokio::sync::Notify::new(),
        });
        let clock = Arc::new(TestClock {
            now: AtomicU64::new(0),
            jitter,
            slept: std::sync::Mutex::new(Vec::new()),
            hold: Some(hold.clone()),
        });
        (clock, hold)
    }

    /// Every delay this clock was asked to wait, in order.
    pub fn waits(&self) -> Vec<Duration> {
        self.slept.lock().expect("test clock").clone()
    }

    /// The total virtual time spent waiting.
    pub fn waited(&self) -> Duration {
        Duration::from_millis(self.now.load(Ordering::SeqCst))
    }
}

#[cfg(test)]
#[async_trait]
impl RetryClock for TestClock {
    fn now_ms(&self) -> u64 {
        self.now.load(Ordering::SeqCst)
    }

    async fn sleep(&self, delay: Duration) {
        self.slept.lock().expect("test clock").push(delay);
        self.now
            .fetch_add(delay.as_millis() as u64, Ordering::SeqCst);
        if let Some(hold) = &self.hold {
            hold.parked.add_permits(1);
            hold.release.notified().await;
        }
    }

    fn jitter(&self) -> f64 {
        self.jitter
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn exhaustion_reports_the_last_transient_failure() {
        let clock = TestClock::new(1.0);
        let calls = AtomicU64::new(0);
        let result: BlobResult<()> = with_retries(
            clock.as_ref(),
            RetryLimits::IDEMPOTENT,
            "GET thing",
            |_| async {
                calls.fetch_add(1, Ordering::SeqCst);
                Attempt::Transient {
                    why: "503".into(),
                    after: None,
                }
            },
        )
        .await;
        assert!(matches!(result, Err(BlobError::Other(message)) if message.contains("503")));
        assert_eq!(
            calls.load(Ordering::SeqCst),
            4,
            "attempt bound not honoured"
        );
        // 100 + 200 + 400 milliseconds at full jitter, well inside 30s.
        assert_eq!(clock.waits().len(), 3);
        assert!(clock.waited() <= RetryLimits::IDEMPOTENT.total);
    }

    #[tokio::test]
    async fn the_elapsed_bound_stops_retrying_before_the_attempt_bound() {
        let clock = TestClock::new(1.0);
        let limits = RetryLimits {
            attempts: 10,
            total: Duration::from_millis(250),
            base: Duration::from_millis(100),
            cap: Duration::from_secs(5),
        };
        let calls = AtomicU64::new(0);
        let result: BlobResult<()> = with_retries(clock.as_ref(), limits, "GET thing", |_| async {
            calls.fetch_add(1, Ordering::SeqCst);
            Attempt::Transient {
                why: "503".into(),
                after: None,
            }
        })
        .await;
        assert!(result.is_err());
        // 100 then 200 would exceed 250 milliseconds, so the third attempt
        // never starts even though seven remain.
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        assert!(clock.waited() <= limits.total);
    }

    #[tokio::test]
    async fn a_provider_delay_longer_than_the_budget_ends_the_operation() {
        let clock = TestClock::new(0.0);
        let limits = RetryLimits {
            attempts: 5,
            total: Duration::from_secs(2),
            base: Duration::from_millis(100),
            cap: Duration::from_secs(5),
        };
        let result: BlobResult<()> = with_retries(clock.as_ref(), limits, "GET thing", |_| async {
            Attempt::Transient {
                why: "slow down".into(),
                after: Some(Duration::from_secs(30)),
            }
        })
        .await;
        assert!(result.is_err());
        assert!(clock.waits().is_empty(), "waited past the elapsed bound");
    }

    #[tokio::test]
    async fn a_provider_delay_raises_the_backoff_but_never_lowers_it() {
        let clock = TestClock::new(0.0);
        let mut window = RetryWindow::new(clock.as_ref(), RetryLimits::IDEMPOTENT);
        assert!(window.begin());
        assert!(window.wait(Some(Duration::from_secs(2))).await);
        assert!(window.begin());
        assert!(window.wait(Some(Duration::from_millis(1))).await);
        assert_eq!(
            clock.waits(),
            vec![Duration::from_secs(2), Duration::from_millis(100)],
            "a provider delay must be a floor, not a replacement"
        );
    }
}
