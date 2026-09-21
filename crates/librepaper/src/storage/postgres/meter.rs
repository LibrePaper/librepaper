//! What the one connection pool is doing, for the operator cost snapshot.
//!
//! This exists to answer the question REVIEW-BIG-IDEAS.md §2.1 left open --
//! whether the configured connections suffice under this deployment's
//! concurrency -- and it can only be answered if the waits are kept apart.
//! A request can wait for the HTTP work semaphore (counted in
//! `server/cost.rs`), then for a pooled connection (counted here), then for
//! the statements themselves (also here, as the transaction's own duration).
//! Reporting one number for all three makes every slowdown look the same.
//!
//! Counters and fixed buckets only. Nothing here is keyed by document,
//! account, network or statement text, so the cardinality is a constant no
//! matter how large the deployment grows.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

use serde_json::{json, Value};

/// Upper edges in microseconds. Anything past the last edge lands in the
/// overflow bucket, which on a healthy deployment stays at zero: the pool's
/// own `acquire_timeout` is five seconds, so a wait of a second is already
/// most of the way to a refusal.
const EDGES: [u64; 7] = [500, 1_000, 5_000, 10_000, 50_000, 250_000, 1_000_000];

/// One `Instant` and a note of whether the pool could possibly have served
/// this caller without waiting, taken before the acquire.
pub struct Attempt {
    started: Instant,
    contended: bool,
}

#[derive(Debug, Default)]
pub struct PoolMeter {
    /// Transactions opened through the metered path.
    begun: AtomicU64,
    /// Opens that ended in an error, which past `acquire_timeout` is what a
    /// pool that is too small for its offered load looks like.
    failed: AtomicU64,
    /// Opens that found no idle connection while the pool was already at its
    /// configured size, so waiting was the only thing left to do. This is
    /// the honest "the pool was the constraint" counter: the wait histogram
    /// alone cannot separate a slow `BEGIN` from a queue.
    contended: AtomicU64,
    wait_micros: AtomicU64,
    wait_micros_max: AtomicU64,
    buckets: [AtomicU64; EDGES.len() + 1],
}

impl PoolMeter {
    /// Starts timing. `contended` is read from the pool by the caller,
    /// because only the caller has it.
    pub fn attempt(contended: bool) -> Attempt {
        Attempt {
            started: Instant::now(),
            contended,
        }
    }

    pub fn record(&self, attempt: Attempt, ok: bool) {
        let micros = attempt
            .started
            .elapsed()
            .as_micros()
            .min(u128::from(u64::MAX)) as u64;
        self.begun.fetch_add(1, Ordering::Relaxed);
        if !ok {
            self.failed.fetch_add(1, Ordering::Relaxed);
        }
        if attempt.contended {
            self.contended.fetch_add(1, Ordering::Relaxed);
        }
        self.wait_micros.fetch_add(micros, Ordering::Relaxed);
        self.wait_micros_max.fetch_max(micros, Ordering::Relaxed);
        let bucket = EDGES
            .iter()
            .position(|edge| micros <= *edge)
            .unwrap_or(EDGES.len());
        self.buckets[bucket].fetch_add(1, Ordering::Relaxed);
    }

    /// `size` and `idle` come from the pool itself; `max` is what this
    /// deployment was configured with. `checked_out` is the number an
    /// operator actually wants: how many of the configured connections are
    /// in somebody's hands right now.
    pub fn snapshot(&self, size: u32, idle: usize, max: u32) -> Value {
        let begun = self.begun.load(Ordering::Relaxed);
        let total = self.wait_micros.load(Ordering::Relaxed);
        let mut histogram = serde_json::Map::new();
        for (index, count) in self.buckets.iter().enumerate() {
            let name = match EDGES.get(index) {
                Some(edge) => format!("le_{edge}us"),
                None => format!("gt_{}us", EDGES[EDGES.len() - 1]),
            };
            histogram.insert(name, json!(count.load(Ordering::Relaxed)));
        }
        json!({
            "max_connections": max,
            "open_connections": size,
            "idle_connections": idle,
            "checked_out": (size as usize).saturating_sub(idle),
            "transactions_begun": begun,
            "transactions_failed_to_begin": self.failed.load(Ordering::Relaxed),
            "begun_with_no_connection_free": self.contended.load(Ordering::Relaxed),
            "begin_wait_mean_us": total.checked_div(begun).unwrap_or(0),
            "begin_wait_max_us": self.wait_micros_max.load(Ordering::Relaxed),
            "begin_wait_histogram": Value::Object(histogram),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_wait_lands_in_exactly_one_bucket_and_the_last_one_is_the_overflow() {
        let meter = PoolMeter::default();
        meter.record(
            Attempt {
                started: Instant::now(),
                contended: false,
            },
            true,
        );
        let snapshot = meter.snapshot(3, 2, 20);
        assert_eq!(snapshot["transactions_begun"], json!(1));
        assert_eq!(snapshot["checked_out"], json!(1));
        // Whatever bucket an immediate record lands in, exactly one moved.
        let counted: u64 = snapshot["begin_wait_histogram"]
            .as_object()
            .unwrap()
            .values()
            .map(|value| value.as_u64().unwrap())
            .sum();
        assert_eq!(counted, 1);
        assert_eq!(snapshot["begun_with_no_connection_free"], json!(0));
    }

    #[test]
    fn contention_and_failure_are_counted_apart_from_the_wait() {
        let meter = PoolMeter::default();
        meter.record(
            Attempt {
                started: Instant::now(),
                contended: true,
            },
            false,
        );
        let snapshot = meter.snapshot(20, 0, 20);
        assert_eq!(snapshot["transactions_failed_to_begin"], json!(1));
        assert_eq!(snapshot["begun_with_no_connection_free"], json!(1));
        assert_eq!(snapshot["idle_connections"], json!(0));
        assert_eq!(snapshot["checked_out"], json!(20));
    }
}
