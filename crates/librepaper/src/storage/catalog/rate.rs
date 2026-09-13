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
}
