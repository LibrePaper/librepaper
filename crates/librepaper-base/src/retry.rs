//! One exponential backoff for every place that tries something again.

use std::future::Future;
use std::time::Duration;

use rand::Rng;

/// How long to wait between tries, and when to stop.
///
/// The wait after the first failure is `base`. It doubles with each further
/// failure and stops growing at `ceiling`. Use `Duration::MAX` for no ceiling
/// and equal `base` and `ceiling` for a constant wait.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RetryPolicy {
    pub base: Duration,
    pub ceiling: Duration,
    /// The most times the operation runs, the first try included. `None` never
    /// stops.
    pub max_attempts: Option<u32>,
    /// Waits a random share, between half and all, of each computed delay.
    pub jitter: bool,
}

impl RetryPolicy {
    /// The wait after `failures` consecutive failures. Zero counts as one.
    pub fn delay(&self, failures: u32) -> Duration {
        let wait = self
            .base
            .saturating_mul(
                1u32.checked_shl(failures.saturating_sub(1))
                    .unwrap_or(u32::MAX),
            )
            .min(self.ceiling);
        if !self.jitter {
            return wait;
        }
        let share = rand::rng().random_range(0.5..=1.0);
        Duration::try_from_secs_f64(wait.as_secs_f64() * share)
            .unwrap_or(wait)
            .min(wait)
    }

    /// Runs `op` until it succeeds, fails with an error `retryable` refuses, or
    /// has run `max_attempts` times. The last error is returned.
    pub async fn run<T, E, Fut>(
        &self,
        mut op: impl FnMut() -> Fut,
        retryable: impl Fn(&E) -> bool,
    ) -> Result<T, E>
    where
        Fut: Future<Output = Result<T, E>>,
    {
        let mut failures = 0u32;
        loop {
            match op().await {
                Ok(value) => return Ok(value),
                Err(error) => {
                    failures = failures.saturating_add(1);
                    let spent = self.max_attempts.is_some_and(|most| failures >= most);
                    if spent || !retryable(&error) {
                        return Err(error);
                    }
                    tokio::time::sleep(self.delay(failures)).await;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    const SECOND: Duration = Duration::from_secs(1);

    fn policy(max_attempts: Option<u32>, jitter: bool) -> RetryPolicy {
        RetryPolicy {
            base: SECOND,
            ceiling: SECOND * 8,
            max_attempts,
            jitter,
        }
    }

    #[test]
    fn the_delay_doubles_and_then_stops_doubling() {
        let policy = policy(None, false);
        let waits: Vec<_> = (0..=6).map(|n| policy.delay(n)).collect();
        let seconds: Vec<_> = waits.iter().map(Duration::as_secs).collect();
        assert_eq!(seconds, [1, 1, 2, 4, 8, 8, 8]);
        assert_eq!(policy.delay(1_000), SECOND * 8);
        assert_eq!(policy.delay(u32::MAX), SECOND * 8);
    }

    #[test]
    fn no_ceiling_saturates_instead_of_overflowing() {
        let policy = RetryPolicy {
            base: SECOND,
            ceiling: Duration::MAX,
            max_attempts: None,
            jitter: false,
        };
        assert_eq!(policy.delay(3), SECOND * 4);
        assert!(policy.delay(u32::MAX) >= SECOND * 1_000);
    }

    #[test]
    fn jitter_stays_between_half_and_all_of_the_delay() {
        let policy = policy(None, true);
        for failures in 1..=6 {
            let full = RetryPolicy {
                jitter: false,
                ..policy
            }
            .delay(failures);
            for _ in 0..200 {
                let wait = policy.delay(failures);
                assert!(wait >= full / 2 && wait <= full, "{wait:?} outside {full:?}");
            }
        }
    }

    #[tokio::test(start_paused = true)]
    async fn max_attempts_stops_the_retries() {
        let policy = policy(Some(3), false);
        let runs = Cell::new(0);
        let result: Result<(), u32> = policy
            .run(
                || {
                    runs.set(runs.get() + 1);
                    std::future::ready(Err(runs.get()))
                },
                |_| true,
            )
            .await;
        assert_eq!(result, Err(3));
        assert_eq!(runs.get(), 3);
    }

    #[tokio::test(start_paused = true)]
    async fn a_non_retryable_error_returns_at_once() {
        let policy = policy(None, false);
        let runs = Cell::new(0);
        let result: Result<(), &str> = policy
            .run(
                || {
                    runs.set(runs.get() + 1);
                    std::future::ready(Err("refused"))
                },
                |_| false,
            )
            .await;
        assert_eq!(result, Err("refused"));
        assert_eq!(runs.get(), 1);
    }

    #[tokio::test(start_paused = true)]
    async fn run_waits_the_computed_delays_and_returns_the_first_success() {
        let policy = policy(None, false);
        let runs = Cell::new(0);
        let start = tokio::time::Instant::now();
        let result: Result<u32, ()> = policy
            .run(
                || {
                    runs.set(runs.get() + 1);
                    std::future::ready(if runs.get() < 4 { Err(()) } else { Ok(runs.get()) })
                },
                |_| true,
            )
            .await;
        assert_eq!(result, Ok(4));
        assert_eq!(start.elapsed(), SECOND * (1 + 2 + 4));
    }
}
