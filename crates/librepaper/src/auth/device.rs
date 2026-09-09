//! The terminal's way in: the device flow this deployment runs itself, the
//! codes it hands out, and the cache that keeps a verified bearer from being
//! checked again on every request.

use std::collections::{HashMap, VecDeque};
use std::future::Future;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use rand::Rng;
use sha2::{Digest, Sha256};
use tokio::sync::{Notify, Semaphore};

use super::{normalized, now_unix, random_bytes, Identity, ProviderError};

/// Keeps bearer tokens from costing a GitHub call per request. Positive
/// answers are cached longer than negative ones, so a token that is revoked or
/// was never valid does not sit trusted for as long as one that is.
pub struct TokenCache {
    pub(super) entries: Mutex<HashMap<String, CachedToken>>,
    pub(super) positive_ttl: Duration,
    pub(super) negative_ttl: Duration,
    in_flight: Mutex<HashMap<String, Arc<TokenFlight>>>,
    provider_slots: Arc<Semaphore>,
    provider_budget: Mutex<VecDeque<Instant>>,
    provider_limit: usize,
    provider_window: Duration,
    provider_cooldown: Mutex<Option<Instant>>,
}

pub(super) struct CachedToken {
    pub(super) identity: Option<Identity>,
    pub(super) expires: Instant,
}

struct TokenFlight {
    result: Mutex<Option<Result<Option<Identity>, ProviderError>>>,
    changed: Notify,
    cancelled: AtomicBool,
}

/// Maximum time a verified identity is trusted without another provider
/// check.
pub const TOKEN_POSITIVE_TTL: Duration = Duration::from_secs(10 * 60);

/// Invalid tokens are cached briefly to prevent a bad bearer from causing a
/// provider request on every retry.
pub const TOKEN_NEGATIVE_TTL: Duration = Duration::from_secs(60);

/// Hard cap on distinct token digests held at once. A deployment ordinarily
/// has at most a few hundred users, so a few thousand entries is generous
/// headroom for legitimate traffic while keeping the worst case -- a flood of
/// distinct invalid bearer tokens -- a small, fixed amount of memory instead
/// of one that grows with an attacker's request rate.
pub const TOKEN_CACHE_CAP: usize = 4096;

/// Maximum number of provider requests allowed to run at once.
pub const TOKEN_CHECK_CONCURRENCY: usize = 16;

/// A provider outage must not turn into an unbounded stream of checks.
pub const TOKEN_CHECK_RATE_LIMIT: usize = 60;
pub const TOKEN_CHECK_RATE_WINDOW: Duration = Duration::from_secs(60);
pub const TOKEN_CHECK_COOLDOWN_FALLBACK: Duration = Duration::from_secs(60);
pub const TOKEN_CHECK_COOLDOWN_MAX: Duration = Duration::from_secs(15 * 60);

/// Distinct token checks admitted to the coalescing table. Calls beyond this
/// fail immediately, without allocating another provider request or cache entry.
pub const TOKEN_IN_FLIGHT_CAP: usize = 256;

impl Default for TokenCache {
    fn default() -> Self {
        Self::new()
    }
}

impl TokenCache {
    pub fn new() -> TokenCache {
        Self::with_config(
            TOKEN_POSITIVE_TTL,
            TOKEN_NEGATIVE_TTL,
            TOKEN_CHECK_RATE_LIMIT,
            TOKEN_CHECK_RATE_WINDOW,
        )
    }

    fn with_config(
        positive_ttl: Duration,
        negative_ttl: Duration,
        provider_limit: usize,
        provider_window: Duration,
    ) -> TokenCache {
        TokenCache {
            entries: Mutex::new(HashMap::new()),
            positive_ttl,
            negative_ttl,
            in_flight: Mutex::new(HashMap::new()),
            provider_slots: Arc::new(Semaphore::new(TOKEN_CHECK_CONCURRENCY)),
            provider_budget: Mutex::new(VecDeque::new()),
            provider_limit,
            provider_window,
            provider_cooldown: Mutex::new(None),
        }
    }

    /// A cache whose lifetimes are configurable, so a test can watch an entry
    /// actually expire and get swept without waiting out the real ten-minute
    /// and one-minute TTLs. Tests also use a large provider budget so a cache
    /// capacity test does not have to wait for the production rate window.
    #[cfg(test)]
    pub fn for_test(positive_ttl: Duration, negative_ttl: Duration) -> TokenCache {
        TokenCache::with_config(
            positive_ttl,
            negative_ttl,
            usize::MAX,
            Duration::from_secs(1),
        )
    }

    /// How many token digests are currently held, for a test to check against
    /// the cap.
    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.entries.lock().expect("token cache poisoned").len()
    }

    /// Resolves a bearer token to an identity, caching only confirmed valid or
    /// invalid answers. Misses for the same token share one provider request;
    /// provider failures are returned to all waiters and are never cached as
    /// invalid. Admission is fail-fast: a full coalescing table, provider
    /// semaphore, or rate budget returns `ProviderError::Busy` rather than
    /// retaining an unbounded queue of request futures.
    pub async fn verify<F, Fut>(&self, check: F, token: &str) -> Result<Identity, ProviderError>
    where
        F: FnOnce(String) -> Fut,
        Fut: Future<Output = Result<Option<Identity>, ProviderError>>,
    {
        if token.is_empty() {
            return Ok(Identity::anonymous());
        }

        let key = hex::encode(Sha256::digest(token.as_bytes()));
        let mut check = Some(check);
        loop {
            if let Some(cached) = self.cached(&key) {
                return Ok(cached.unwrap_or_default());
            }

            let (flight, leader) = {
                // Recheck entries while holding the admission lock. A caller
                // can have observed a miss just before a leader completed;
                // this second check prevents that caller from starting a
                // duplicate provider request after the flight is removed.
                let mut in_flight = self.in_flight.lock().expect("token flights poisoned");
                let entries = self.entries.lock().expect("token cache poisoned");
                if let Some(entry) = entries.get(&key) {
                    if Instant::now() < entry.expires {
                        return Ok(entry.identity.clone().unwrap_or_default());
                    }
                }
                drop(entries);
                if let Some(flight) = in_flight.get(&key) {
                    (Arc::clone(flight), false)
                } else if in_flight.len() >= TOKEN_IN_FLIGHT_CAP {
                    return Err(ProviderError::Busy);
                } else {
                    let flight = Arc::new(TokenFlight {
                        result: Mutex::new(None),
                        changed: Notify::new(),
                        cancelled: AtomicBool::new(false),
                    });
                    in_flight.insert(key.clone(), Arc::clone(&flight));
                    (flight, true)
                }
            };

            if leader {
                let mut owner = FlightOwner {
                    cache: self,
                    key: key.clone(),
                    flight: Arc::clone(&flight),
                    finished: false,
                };
                let result = self
                    .check_once(
                        check.take().expect("token check closure already consumed"),
                        token,
                    )
                    .await;
                // Keep the flight in the map while publishing a successful
                // cache answer. New callers therefore see either the flight
                // or the completed cache entry, never an uncovered gap.
                let mut in_flight = self.in_flight.lock().expect("token flights poisoned");
                if let Ok(identity) = &result {
                    self.cache_answer(&key, identity.clone());
                }
                *flight.result.lock().expect("token flight poisoned") = Some(result.clone());
                if in_flight
                    .get(&key)
                    .is_some_and(|current| Arc::ptr_eq(current, &flight))
                {
                    in_flight.remove(&key);
                }
                drop(in_flight);
                flight.changed.notify_waiters();
                owner.finished = true;
                return result.map(|identity| identity.unwrap_or_default());
            }

            let notified = flight.changed.notified();
            if let Some(result) = flight.result.lock().expect("token flight poisoned").clone() {
                return result.map(|identity| identity.unwrap_or_default());
            }
            if flight.cancelled.load(Ordering::Acquire) {
                continue;
            }
            notified.await;
            let result = flight.result.lock().expect("token flight poisoned").clone();
            if let Some(result) = result {
                return result.map(|identity| identity.unwrap_or_default());
            }
            // A cancelled leader removes the flight without setting a result.
            // Re-entering the loop lets this waiter become the new leader.
        }
    }

    fn cached(&self, key: &str) -> Option<Option<Identity>> {
        let entries = self.entries.lock().expect("token cache poisoned");
        entries
            .get(key)
            .and_then(|entry| (Instant::now() < entry.expires).then(|| entry.identity.clone()))
    }

    async fn check_once<F, Fut>(
        &self,
        check: F,
        token: &str,
    ) -> Result<Option<Identity>, ProviderError>
    where
        F: FnOnce(String) -> Fut,
        Fut: Future<Output = Result<Option<Identity>, ProviderError>>,
    {
        if !self.provider_cooldown_expired() {
            return Err(ProviderError::Busy);
        }
        let permit = self
            .provider_slots
            .clone()
            .try_acquire_owned()
            .map_err(|_| ProviderError::Busy)?;
        if !self.try_reserve_provider_check() {
            return Err(ProviderError::Busy);
        }
        let result = check(token.to_string()).await;
        drop(permit);
        if let Err(ProviderError::RateLimited { retry_after, .. }) = &result {
            self.set_provider_cooldown(*retry_after);
        }
        result
    }

    fn provider_cooldown_expired(&self) -> bool {
        let mut cooldown = self
            .provider_cooldown
            .lock()
            .expect("token cooldown poisoned");
        match *cooldown {
            Some(deadline) if deadline > Instant::now() => false,
            Some(_) => {
                *cooldown = None;
                true
            }
            None => true,
        }
    }

    fn set_provider_cooldown(&self, retry_after: Option<Duration>) {
        let delay = retry_after
            .unwrap_or(TOKEN_CHECK_COOLDOWN_FALLBACK)
            .max(TOKEN_CHECK_COOLDOWN_FALLBACK)
            .min(TOKEN_CHECK_COOLDOWN_MAX);
        let deadline = Instant::now() + delay;
        let mut cooldown = self
            .provider_cooldown
            .lock()
            .expect("token cooldown poisoned");
        if cooldown.is_none_or(|existing| existing < deadline) {
            *cooldown = Some(deadline);
        }
    }

    fn try_reserve_provider_check(&self) -> bool {
        let mut budget = self.provider_budget.lock().expect("token budget poisoned");
        let now = Instant::now();
        while budget
            .front()
            .is_some_and(|oldest| now.duration_since(*oldest) >= self.provider_window)
        {
            budget.pop_front();
        }
        if budget.len() >= self.provider_limit {
            return false;
        }
        budget.push_back(now);
        true
    }

    fn cache_answer(&self, key: &str, identity: Option<Identity>) {
        let ttl = if identity.is_some() {
            self.positive_ttl
        } else {
            self.negative_ttl
        };
        let mut entries = self.entries.lock().expect("token cache poisoned");
        let now = Instant::now();
        entries.retain(|_, cached| cached.expires > now);
        while entries.len() >= TOKEN_CACHE_CAP {
            let Some(soonest) = entries
                .iter()
                .min_by_key(|(_, cached)| cached.expires)
                .map(|(key, _)| key.clone())
            else {
                break;
            };
            entries.remove(&soonest);
        }
        entries.insert(
            key.to_string(),
            CachedToken {
                identity,
                expires: now + ttl,
            },
        );
    }

    fn remove_flight(&self, key: &str, flight: &Arc<TokenFlight>) {
        let mut in_flight = self.in_flight.lock().expect("token flights poisoned");
        if in_flight
            .get(key)
            .is_some_and(|current| Arc::ptr_eq(current, flight))
        {
            in_flight.remove(key);
        }
    }
}

struct FlightOwner<'a> {
    cache: &'a TokenCache,
    key: String,
    flight: Arc<TokenFlight>,
    finished: bool,
}

impl Drop for FlightOwner<'_> {
    fn drop(&mut self) {
        if self.finished {
            return;
        }
        self.flight.cancelled.store(true, Ordering::Release);
        self.cache.remove_flight(&self.key, &self.flight);
        self.flight.changed.notify_waiters();
    }
}

/// How long a pending sign-in from a terminal lives before it is forgotten.
pub const DEVICE_CODE_MAX_AGE: Duration = Duration::from_secs(10 * 60);

/// How long the token the flow hands out lives.
pub const DEVICE_TOKEN_MAX_AGE: Duration = Duration::from_secs(90 * 24 * 3600);

/// What the CLI is told to wait between polls.
pub const DEVICE_POLL_INTERVAL: u64 = 2;

/// An `lp_` bearer is this deployment's own token.
pub const DEVICE_TOKEN_PREFIX: &str = "lp_";

/// The alphabet a user code is read aloud and typed from.
pub(super) const USER_CODE_ALPHABET: &[u8] = b"ABCDEFGHJKMNPQRSTUVWXYZ23456789";

pub(super) const USER_CODE_LENGTH: usize = 8;

/// How many sign-ins may be pending at once.
pub const DEVICE_CODES_MAX: usize = 1000;

/// A source may begin at most this many device flows in one pending-code
/// lifetime. The server supplies a trusted peer-derived key.
pub const DEVICE_SOURCE_STARTS_MAX: usize = 10;
pub const DEVICE_SOURCE_WINDOW: Duration = DEVICE_CODE_MAX_AGE;

/// Source limiter state is deliberately non-evicting. Once this bounded table
/// is full, a new source is refused until an old window expires; an attacker
/// cannot churn unknown keys to evict an established source and reset its
/// allowance.
pub const DEVICE_SOURCE_KEYS_MAX: usize = 4096;

/// Eight characters selected uniformly from the human-readable alphabet.
pub fn user_code() -> String {
    let mut rng = rand::rng();
    (0..USER_CODE_LENGTH)
        .map(|_| {
            let index = rng.random_range(0..USER_CODE_ALPHABET.len());
            USER_CODE_ALPHABET[index] as char
        })
        .collect()
}

/// 128 random bits, which is what the terminal holds and never shows.
pub fn device_code() -> String {
    hex::encode(random_bytes(16))
}

pub(super) fn digest_of(value: &str) -> String {
    hex::encode(Sha256::digest(value.as_bytes()))
}

/// One terminal waiting to be signed in.
pub(super) struct Pending {
    pub(super) device_digest: String,
    pub(super) created: i64,
    pub(super) approved: Option<Identity>,
}

struct SourceWindow {
    started: Instant,
    starts: usize,
}

/// The pending sign-ins, keyed by the user code the person types.
pub struct PendingCodes {
    pub(super) entries: Mutex<HashMap<String, Pending>>,
    source_limits: Mutex<HashMap<String, SourceWindow>>,
    /// Seconds, so a test can shorten it through a shared reference.
    pub(super) max_age: AtomicU64,
}

/// What a poll for the token learns.
pub enum DeviceOutcome {
    Pending,
    Expired,
    Approved(Identity),
}

impl Default for PendingCodes {
    fn default() -> Self {
        Self::new()
    }
}

impl PendingCodes {
    pub fn new() -> PendingCodes {
        PendingCodes {
            entries: Mutex::new(HashMap::new()),
            source_limits: Mutex::new(HashMap::new()),
            max_age: AtomicU64::new(DEVICE_CODE_MAX_AGE.as_secs()),
        }
    }

    pub fn max_age(&self) -> i64 {
        self.max_age.load(Ordering::Relaxed) as i64
    }

    #[allow(dead_code)]
    pub fn set_max_age(&self, seconds: u64) {
        self.max_age.store(seconds, Ordering::Relaxed);
    }

    /// Starts a flow after applying both the global cap and the source window.
    /// The source key is hashed before being stored, so even an unexpectedly
    /// large peer key cannot turn the limiter into an unbounded memory sink.
    pub fn start_for(&self, rate_key: &str) -> Option<(String, String)> {
        if rate_key.is_empty() {
            return None;
        }
        let mut entries = self.entries.lock().expect("pending codes poisoned");
        self.sweep(&mut entries);
        if entries.len() >= DEVICE_CODES_MAX {
            return None;
        }

        let now = Instant::now();
        let source_key = digest_of(rate_key);
        let mut source_limits = self.source_limits.lock().expect("source limiter poisoned");
        source_limits.retain(|_, window| now.duration_since(window.started) < DEVICE_SOURCE_WINDOW);
        let Some(window) = source_limits.get_mut(&source_key) else {
            if source_limits.len() >= DEVICE_SOURCE_KEYS_MAX {
                return None;
            }
            source_limits.insert(
                source_key,
                SourceWindow {
                    started: now,
                    starts: 1,
                },
            );
            return self.insert_pending(&mut entries);
        };
        if window.starts >= DEVICE_SOURCE_STARTS_MAX {
            return None;
        }
        window.starts += 1;
        self.insert_pending(&mut entries)
    }

    /// Test-only compatibility helper for filling the global table without
    /// making one synthetic source hit the production source limit.
    #[cfg(test)]
    pub fn start(&self) -> Option<(String, String)> {
        let mut entries = self.entries.lock().expect("pending codes poisoned");
        self.sweep(&mut entries);
        if entries.len() >= DEVICE_CODES_MAX {
            return None;
        }
        self.insert_pending(&mut entries)
    }

    fn insert_pending(&self, entries: &mut HashMap<String, Pending>) -> Option<(String, String)> {
        for _ in 0..4 {
            let user = user_code();
            if entries.contains_key(&user) {
                continue;
            }
            let device = device_code();
            entries.insert(
                user.clone(),
                Pending {
                    device_digest: digest_of(&device),
                    created: now_unix(),
                    approved: None,
                },
            );
            return Some((device, user));
        }
        None
    }

    /// Binds an identity to a pending code. Approval is one-way: a retry by
    /// the same account succeeds without replacing the first identity (and in
    /// particular without refreshing its session generation), while another
    /// account cannot overwrite it.
    pub fn approve(&self, user: &str, who: &Identity) -> bool {
        let mut entries = self.entries.lock().expect("pending codes poisoned");
        self.sweep(&mut entries);
        match entries.get_mut(&normalized(user)) {
            Some(entry) => match &entry.approved {
                None => {
                    entry.approved = Some(who.clone());
                    true
                }
                Some(existing) => existing.provider == who.provider && existing.id == who.id,
            },
            None => false,
        }
    }

    /// What the terminal's poll gets. An approved entry is removed as it is
    /// read, so the token is handed out exactly once.
    pub fn claim(&self, device: &str) -> DeviceOutcome {
        let mut entries = self.entries.lock().expect("pending codes poisoned");
        self.sweep(&mut entries);
        let digest = digest_of(device);
        let Some(user) = entries
            .iter()
            .find(|(_, entry)| entry.device_digest == digest)
            .map(|(user, _)| user.clone())
        else {
            return DeviceOutcome::Expired;
        };
        match entries.get(&user).and_then(|entry| entry.approved.clone()) {
            Some(who) => {
                entries.remove(&user);
                DeviceOutcome::Approved(who)
            }
            None => DeviceOutcome::Pending,
        }
    }

    #[allow(dead_code)]
    pub fn len(&self) -> usize {
        let mut entries = self.entries.lock().expect("pending codes poisoned");
        self.sweep(&mut entries);
        entries.len()
    }

    pub(super) fn sweep(&self, entries: &mut HashMap<String, Pending>) {
        let cutoff = now_unix() - self.max_age();
        entries.retain(|_, entry| entry.created > cutoff);
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    use tokio::sync::Notify;

    use super::{
        digest_of, user_code, DeviceOutcome, PendingCodes, ProviderError, SourceWindow, TokenCache,
        DEVICE_SOURCE_KEYS_MAX, DEVICE_SOURCE_STARTS_MAX, TOKEN_CHECK_CONCURRENCY,
        TOKEN_IN_FLIGHT_CAP,
    };
    use crate::auth::Identity;

    #[test]
    fn source_admission_is_bounded_without_eviction() {
        let pending = PendingCodes::new();
        let first = "trusted-source";
        for _ in 0..DEVICE_SOURCE_STARTS_MAX {
            assert!(pending.start_for(first).is_some());
        }
        assert!(pending.start_for(first).is_none());
        for index in 0..super::DEVICE_SOURCE_KEYS_MAX {
            let _ = pending.start_for(&format!("unknown-{index}"));
        }
        assert!(
            pending
                .source_limits
                .lock()
                .expect("source limiter poisoned")
                .len()
                <= super::DEVICE_SOURCE_KEYS_MAX
        );
        assert!(pending.start_for(first).is_none());
    }

    #[test]
    fn full_source_limiter_refuses_unknown_keys_without_eviction() {
        let pending = PendingCodes::new();
        let now = Instant::now();
        let trusted = digest_of("trusted-source");
        let mut source_limits = pending
            .source_limits
            .lock()
            .expect("source limiter poisoned");
        source_limits.insert(
            trusted.clone(),
            SourceWindow {
                started: now,
                starts: DEVICE_SOURCE_STARTS_MAX,
            },
        );
        for index in 0..(DEVICE_SOURCE_KEYS_MAX - 1) {
            source_limits.insert(
                format!("synthetic-source-{index}"),
                SourceWindow {
                    started: now,
                    starts: 1,
                },
            );
        }
        assert_eq!(source_limits.len(), DEVICE_SOURCE_KEYS_MAX);
        drop(source_limits);

        assert!(pending.start_for("trusted-source").is_none());
        assert!(pending.start_for("new-unknown-source").is_none());
        assert_eq!(
            pending
                .source_limits
                .lock()
                .expect("source limiter poisoned")
                .len(),
            DEVICE_SOURCE_KEYS_MAX
        );
    }

    #[test]
    fn approval_is_idempotent_for_the_same_account() {
        let pending = PendingCodes::new();
        let (device, user) = pending.start_for("source").expect("start admitted");
        let first = Identity {
            session_generation: "old-generation".into(),
            ..Identity::github("alice", "1")
        };
        let retry = Identity {
            session_generation: "new-generation".into(),
            name: "renamed".into(),
            ..Identity::github("alice", "1")
        };
        let other = Identity::github("bob", "2");
        assert!(pending.approve(&user, &first));
        assert!(pending.approve(&user, &retry));
        assert!(!pending.approve(&user, &other));
        match pending.claim(&device) {
            DeviceOutcome::Approved(identity) => {
                assert_eq!(identity.session_generation, "old-generation");
                assert_eq!(identity.name, "alice");
            }
            _ => panic!("approved device was not claimable"),
        }
    }

    #[test]
    fn user_codes_use_only_the_declared_alphabet() {
        for _ in 0..100 {
            assert!(user_code()
                .bytes()
                .all(|byte| super::USER_CODE_ALPHABET.contains(&byte)));
        }
    }

    #[tokio::test]
    async fn concurrent_token_misses_are_coalesced() {
        let cache = Arc::new(TokenCache::for_test(
            Duration::from_secs(60),
            Duration::from_secs(60),
        ));
        let calls = Arc::new(AtomicUsize::new(0));
        let started = Arc::new(Notify::new());
        let release = Arc::new(Notify::new());

        let leader_cache = Arc::clone(&cache);
        let leader_calls = Arc::clone(&calls);
        let leader_started = Arc::clone(&started);
        let leader_release = Arc::clone(&release);
        let leader = tokio::spawn(async move {
            leader_cache
                .verify(
                    move |_| {
                        leader_calls.fetch_add(1, Ordering::SeqCst);
                        leader_started.notify_one();
                        let release = Arc::clone(&leader_release);
                        async move {
                            release.notified().await;
                            Ok::<_, ProviderError>(None)
                        }
                    },
                    "same-token",
                )
                .await
        });
        started.notified().await;

        let mut followers = Vec::new();
        for _ in 0..(TOKEN_IN_FLIGHT_CAP / 2) {
            let cache = Arc::clone(&cache);
            followers.push(tokio::spawn(async move {
                cache
                    .verify(|_| async { Ok::<_, ProviderError>(None) }, "same-token")
                    .await
            }));
        }
        release.notify_waiters();
        assert!(!leader
            .await
            .expect("leader panicked")
            .expect("check failed")
            .is_signed_in());
        for follower in followers {
            assert!(!follower
                .await
                .expect("follower panicked")
                .expect("check failed")
                .is_signed_in());
        }
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn concurrent_transient_misses_share_one_error() {
        let cache = Arc::new(TokenCache::for_test(
            Duration::from_secs(60),
            Duration::from_secs(60),
        ));
        let calls = Arc::new(AtomicUsize::new(0));
        let started = Arc::new(Notify::new());
        let release = Arc::new(Notify::new());
        let leader_cache = Arc::clone(&cache);
        let leader_calls = Arc::clone(&calls);
        let leader_started = Arc::clone(&started);
        let leader_release = Arc::clone(&release);
        let leader = tokio::spawn(async move {
            leader_cache
                .verify(
                    move |_| {
                        leader_calls.fetch_add(1, Ordering::SeqCst);
                        leader_started.notify_one();
                        let release = Arc::clone(&leader_release);
                        async move {
                            release.notified().await;
                            Err::<Option<Identity>, _>(ProviderError::Upstream { status: 503 })
                        }
                    },
                    "transient-shared-token",
                )
                .await
        });
        started.notified().await;
        let mut followers = Vec::new();
        for _ in 0..32 {
            let cache = Arc::clone(&cache);
            followers.push(tokio::spawn(async move {
                cache
                    .verify(
                        |_| async { Err::<Option<Identity>, _>(ProviderError::Busy) },
                        "transient-shared-token",
                    )
                    .await
            }));
        }
        release.notify_waiters();
        assert_eq!(
            leader.await.expect("leader panicked").unwrap_err(),
            ProviderError::Upstream { status: 503 }
        );
        for follower in followers {
            assert_eq!(
                follower.await.expect("follower panicked").unwrap_err(),
                ProviderError::Upstream { status: 503 }
            );
        }
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn provider_calls_never_exceed_the_concurrency_bound() {
        let cache = Arc::new(TokenCache::with_config(
            Duration::from_secs(60),
            Duration::from_secs(60),
            usize::MAX,
            Duration::from_secs(1),
        ));
        let active = Arc::new(AtomicUsize::new(0));
        let maximum = Arc::new(AtomicUsize::new(0));
        let calls = Arc::new(AtomicUsize::new(0));
        let mut tasks = Vec::new();
        for index in 0..(TOKEN_CHECK_CONCURRENCY * 4) {
            let cache = Arc::clone(&cache);
            let active = Arc::clone(&active);
            let maximum = Arc::clone(&maximum);
            let calls = Arc::clone(&calls);
            tasks.push(tokio::spawn(async move {
                let token = format!("concurrency-token-{index}");
                cache
                    .verify(
                        move |_| {
                            calls.fetch_add(1, Ordering::SeqCst);
                            let current = active.fetch_add(1, Ordering::SeqCst) + 1;
                            maximum.fetch_max(current, Ordering::SeqCst);
                            let active = Arc::clone(&active);
                            async move {
                                tokio::time::sleep(Duration::from_millis(20)).await;
                                active.fetch_sub(1, Ordering::SeqCst);
                                Ok::<_, ProviderError>(None)
                            }
                        },
                        &token,
                    )
                    .await
            }));
        }
        for task in tasks {
            let _ = task.await.expect("provider task panicked");
        }
        assert!(calls.load(Ordering::SeqCst) <= TOKEN_CHECK_CONCURRENCY);
        assert!(maximum.load(Ordering::SeqCst) <= TOKEN_CHECK_CONCURRENCY);
        assert_eq!(active.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn provider_rate_budget_fails_new_checks_without_queueing() {
        let cache = TokenCache::with_config(
            Duration::from_secs(60),
            Duration::from_secs(60),
            2,
            Duration::from_secs(60),
        );
        let calls = Arc::new(AtomicUsize::new(0));
        for index in 0..2 {
            let calls = Arc::clone(&calls);
            let identity = cache
                .verify(
                    move |_| {
                        calls.fetch_add(1, Ordering::SeqCst);
                        async { Ok::<_, ProviderError>(None) }
                    },
                    &format!("budget-token-{index}"),
                )
                .await
                .expect("the initial budget should admit two checks");
            assert!(!identity.is_signed_in());
        }
        let rejected = cache
            .verify(
                |_| async { panic!("a rate-budget rejection queued provider work") },
                "budget-token-rejected",
            )
            .await;
        assert_eq!(rejected, Err(ProviderError::Busy));
        assert_eq!(calls.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn cancelled_leader_does_not_leave_a_stale_flight() {
        let cache = Arc::new(TokenCache::for_test(
            Duration::from_secs(60),
            Duration::from_secs(60),
        ));
        let started = Arc::new(Notify::new());
        let never = Arc::new(Notify::new());
        let leader_cache = Arc::clone(&cache);
        let leader_started = Arc::clone(&started);
        let leader_never = Arc::clone(&never);
        let leader = tokio::spawn(async move {
            leader_cache
                .verify(
                    move |_| {
                        leader_started.notify_one();
                        let never = Arc::clone(&leader_never);
                        async move {
                            never.notified().await;
                            Ok::<_, ProviderError>(None)
                        }
                    },
                    "cancelled-token",
                )
                .await
        });
        started.notified().await;
        leader.abort();
        let _ = leader.await;

        let result = tokio::time::timeout(
            Duration::from_secs(1),
            cache.verify(
                |_| async { Ok::<_, ProviderError>(None) },
                "cancelled-token",
            ),
        )
        .await
        .expect("a cancelled flight left a waiter blocked")
        .expect("replacement check failed");
        assert!(!result.is_signed_in());
    }

    #[tokio::test]
    async fn transient_provider_failure_is_not_cached() {
        let cache = TokenCache::for_test(Duration::from_secs(60), Duration::from_secs(60));
        let first = cache
            .verify(
                |_| async { Err::<Option<Identity>, _>(ProviderError::Upstream { status: 503 }) },
                "transient-token",
            )
            .await;
        assert_eq!(first, Err(ProviderError::Upstream { status: 503 }));
        assert_eq!(cache.len(), 0);

        let second = cache
            .verify(
                |_| async { Ok::<_, ProviderError>(None) },
                "transient-token",
            )
            .await
            .expect("the next check should be admitted");
        assert!(!second.is_signed_in());
        assert_eq!(cache.len(), 1);
    }

    #[tokio::test]
    async fn rate_limit_sets_a_shared_cooldown_for_new_tokens() {
        let cache = TokenCache::for_test(Duration::from_secs(60), Duration::from_secs(60));
        let first = cache
            .verify(
                |_| async {
                    Err::<Option<Identity>, _>(ProviderError::RateLimited {
                        status: 429,
                        retry_after: Some(Duration::ZERO),
                    })
                },
                "rate-limited-token",
            )
            .await;
        assert_eq!(
            first,
            Err(ProviderError::RateLimited {
                status: 429,
                retry_after: Some(Duration::ZERO)
            })
        );

        let calls = Arc::new(AtomicUsize::new(0));
        let calls_again = Arc::clone(&calls);
        let second = cache
            .verify(
                move |_| {
                    calls_again.fetch_add(1, Ordering::SeqCst);
                    async { Ok::<_, ProviderError>(None) }
                },
                "a-different-token",
            )
            .await;
        assert_eq!(second, Err(ProviderError::Busy));
        assert_eq!(calls.load(Ordering::SeqCst), 0);
    }
}
