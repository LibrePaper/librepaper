//! Bounded process-local token buckets for mutation admission.
//!
//! Rate state deliberately lives outside SQLite.  A restart resets the
//! buckets, while the entry bound and idle expiry keep attacker-controlled
//! owner identities from becoming an unbounded in-memory index.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use super::{CatalogError, CatalogRefusal, CatalogResult};

const MAX_ENTRIES: usize = 4096;
const IDLE_EXPIRY: Duration = Duration::from_secs(2 * 60 * 60);
const WINDOW: Duration = Duration::from_secs(60 * 60);

#[derive(Debug)]
struct Bucket {
    tokens: f64,
    sampled: Instant,
    limit: usize,
}

#[derive(Default)]
pub(crate) struct ProcessRateState {
    buckets: HashMap<String, Bucket>,
}

/// One reserved mutation token.  A failed or cancelled mutation returns its
/// token; callers mark it consumed only after the durable operation commits.
pub(crate) struct ProcessRateReservation {
    state: Arc<Mutex<ProcessRateState>>,
    key: String,
    committed: bool,
}

impl ProcessRateReservation {
    pub(crate) fn commit(&mut self) {
        self.committed = true;
    }
}

impl Drop for ProcessRateReservation {
    fn drop(&mut self) {
        if self.committed {
            return;
        }
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        if let Some(bucket) = state.buckets.get_mut(&self.key) {
            bucket.tokens = (bucket.tokens + 1.0).min(bucket.limit as f64);
        }
    }
}

pub(crate) fn reserve(
    state: &Arc<Mutex<ProcessRateState>>,
    owner: &str,
    action: &str,
    limit: usize,
) -> CatalogResult<ProcessRateReservation> {
    if limit == 0 || owner.is_empty() || action.is_empty() {
        return Err(CatalogError::refused(
            CatalogRefusal::UploadRate,
            "mutation rate limit exceeded",
        ));
    }
    let key = format!("{action}\0{owner}");
    let now = Instant::now();
    let mut buckets = state.lock().unwrap_or_else(|poison| poison.into_inner());
    buckets
        .buckets
        .retain(|_, bucket| now.duration_since(bucket.sampled) < IDLE_EXPIRY);
    if !buckets.buckets.contains_key(&key) && buckets.buckets.len() >= MAX_ENTRIES {
        return Err(CatalogError::refused(
            CatalogRefusal::UploadRate,
            "mutation rate limiter is full",
        ));
    }
    let bucket = buckets.buckets.entry(key.clone()).or_insert(Bucket {
        tokens: limit as f64,
        sampled: now,
        limit,
    });
    bucket.limit = limit;
    let elapsed = now.duration_since(bucket.sampled).as_secs_f64();
    bucket.tokens =
        (bucket.tokens + elapsed * limit as f64 / WINDOW.as_secs_f64()).min(limit as f64);
    bucket.sampled = now;
    if bucket.tokens < 1.0 {
        return Err(CatalogError::refused(
            CatalogRefusal::UploadRate,
            "mutation rate limit exceeded",
        ));
    }
    bucket.tokens -= 1.0;
    drop(buckets);
    Ok(ProcessRateReservation {
        state: Arc::clone(state),
        key,
        committed: false,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn failed_mutation_returns_its_token() {
        let state = Arc::new(Mutex::new(ProcessRateState::default()));
        let reservation = reserve(&state, "account:a", "source_upload", 1).unwrap();
        assert!(reserve(&state, "account:a", "source_upload", 1).is_err());
        drop(reservation);
        let mut committed = reserve(&state, "account:a", "source_upload", 1).unwrap();
        committed.commit();
        assert!(reserve(&state, "account:a", "source_upload", 1).is_err());
    }

    #[test]
    fn owner_and_action_names_are_independent_buckets() {
        let state = Arc::new(Mutex::new(ProcessRateState::default()));
        let owner = reserve(&state, "account:a", "source_upload", 1).unwrap();
        assert!(reserve(&state, "account:b", "source_upload", 1).is_ok());
        assert!(reserve(&state, "account:a", "checkpoint", 1).is_ok());
        drop(owner);
    }

    #[test]
    fn committed_reservation_does_not_reset_at_window_boundary() {
        let state = Arc::new(Mutex::new(ProcessRateState::default()));
        let mut first = reserve(&state, "account:boundary", "source_upload", 1).unwrap();
        first.commit();
        {
            let mut state = state.lock().unwrap();
            let bucket = state
                .buckets
                .get_mut("source_upload\0account:boundary")
                .unwrap();
            // A monotonic sample crossing the nominal window boundary refills
            // continuously. It must not create a fresh fixed window merely
            // because a wall-clock hour changed.
            bucket.sampled = Instant::now() - WINDOW;
        }
        let mut at_boundary = reserve(&state, "account:boundary", "source_upload", 1).unwrap();
        at_boundary.commit();
        assert!(reserve(&state, "account:boundary", "source_upload", 1).is_err());
    }

    #[test]
    fn refill_is_fractional_and_continuous() {
        let state = Arc::new(Mutex::new(ProcessRateState::default()));
        for _ in 0..4 {
            let mut reservation = reserve(&state, "account:fraction", "source_upload", 4).unwrap();
            reservation.commit();
        }
        {
            let mut state = state.lock().unwrap();
            let bucket = state
                .buckets
                .get_mut("source_upload\0account:fraction")
                .unwrap();
            bucket.sampled = Instant::now() - WINDOW / 8;
        }
        assert!(reserve(&state, "account:fraction", "source_upload", 4).is_err());
        let fractional_tokens = state
            .lock()
            .unwrap()
            .buckets
            .get("source_upload\0account:fraction")
            .unwrap()
            .tokens;
        assert!(fractional_tokens > 0.45 && fractional_tokens < 0.60);

        {
            let mut state = state.lock().unwrap();
            let bucket = state
                .buckets
                .get_mut("source_upload\0account:fraction")
                .unwrap();
            bucket.sampled = Instant::now() - WINDOW / 8;
        }
        let mut refilled = reserve(&state, "account:fraction", "source_upload", 4).unwrap();
        refilled.commit();
    }

    #[test]
    fn dropped_reservation_refunds_original_owner_and_action() {
        let state = Arc::new(Mutex::new(ProcessRateState::default()));
        let original = reserve(&state, "account:original", "source_upload", 1).unwrap();
        let mut unrelated_owner = reserve(&state, "account:unrelated", "source_upload", 1).unwrap();
        unrelated_owner.commit();
        let mut unrelated_action = reserve(&state, "account:original", "checkpoint", 1).unwrap();
        unrelated_action.commit();

        drop(original);
        let mut refunded = reserve(&state, "account:original", "source_upload", 1).unwrap();
        refunded.commit();
        assert!(reserve(&state, "account:original", "checkpoint", 1).is_err());
    }

    #[test]
    fn expired_entries_make_room_without_exceeding_bound() {
        let state = Arc::new(Mutex::new(ProcessRateState::default()));
        let now = Instant::now();
        {
            let mut state = state.lock().unwrap();
            for index in 0..MAX_ENTRIES {
                state.buckets.insert(
                    format!("source_upload\0account:{index}"),
                    Bucket {
                        tokens: 1.0,
                        sampled: now,
                        limit: 1,
                    },
                );
            }
        }
        assert!(reserve(&state, "account:new", "source_upload", 1).is_err());
        {
            let mut state = state.lock().unwrap();
            let expired_at = Instant::now() - IDLE_EXPIRY;
            for bucket in state.buckets.values_mut() {
                bucket.sampled = expired_at;
            }
        }
        let mut reservation = reserve(&state, "account:new", "source_upload", 1).unwrap();
        reservation.commit();
        assert_eq!(state.lock().unwrap().buckets.len(), 1);
    }
}
