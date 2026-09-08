//! The terminal's way in: the device flow this deployment runs itself, the
//! codes it hands out, and the cache that keeps a verified bearer from being
//! checked again on every request.

use super::*;

/// Keeps bearer tokens from costing a GitHub call per request. Positive
/// answers are cached longer than negative ones, so a token that is revoked or
/// was never valid does not sit trusted for as long as one that is.
///
/// Every distinct token this server is ever shown earns an entry, including
/// one nobody will present again -- such as one of a stream of invalid bearer
/// tokens from an attacker. Nothing here relies on the same token being
/// looked up often enough to make an LRU worthwhile, so the cache instead
/// bounds itself by sweeping what has expired on every insert and, failing
/// that, evicting whatever is closest to expiring anyway.
pub struct TokenCache {
    pub(super) entries: Mutex<HashMap<String, CachedToken>>,
    pub(super) positive_ttl: Duration,
    pub(super) negative_ttl: Duration,
}

pub(super) struct CachedToken {
    pub(super) identity: Option<Identity>,
    pub(super) expires: Instant,
}

pub const TOKEN_POSITIVE_TTL: Duration = Duration::from_secs(10 * 60);

pub const TOKEN_NEGATIVE_TTL: Duration = Duration::from_secs(60);

/// Hard cap on distinct token digests held at once. A deployment ordinarily
/// has at most a few hundred users, so a few thousand entries is generous
/// headroom for legitimate traffic while keeping the worst case -- a flood of
/// distinct invalid bearer tokens -- a small, fixed amount of memory instead
/// of one that grows with an attacker's request rate.
pub const TOKEN_CACHE_CAP: usize = 4096;

impl Default for TokenCache {
    fn default() -> Self {
        TokenCache::new()
    }
}

impl TokenCache {
    pub fn new() -> TokenCache {
        TokenCache::with_ttls(TOKEN_POSITIVE_TTL, TOKEN_NEGATIVE_TTL)
    }

    pub(super) fn with_ttls(positive_ttl: Duration, negative_ttl: Duration) -> TokenCache {
        TokenCache {
            entries: Mutex::new(HashMap::new()),
            positive_ttl,
            negative_ttl,
        }
    }

    /// A cache whose lifetimes are configurable, so a test can watch an entry
    /// actually expire and get swept without waiting out the real ten-minute
    /// and one-minute TTLs production uses.
    #[cfg(test)]
    pub fn for_test(positive_ttl: Duration, negative_ttl: Duration) -> TokenCache {
        TokenCache::with_ttls(positive_ttl, negative_ttl)
    }

    /// How many token digests are currently held, for a test to check against
    /// the cap.
    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.entries.lock().expect("token cache poisoned").len()
    }

    /// Resolves a bearer token to an identity, caching the answer keyed by a
    /// digest of the token -- never the token itself. `check` is what actually
    /// asks GitHub; a test substitutes a stand-in with the same shape so the
    /// caching behaviour can be exercised without a network call.
    pub async fn verify<F, Fut>(&self, check: F, token: &str) -> Identity
    where
        F: FnOnce(String) -> Fut,
        Fut: std::future::Future<Output = Option<Identity>>,
    {
        if token.is_empty() {
            return Identity::anonymous();
        }
        let key = hex::encode(Sha256::digest(token.as_bytes()));
        {
            let entries = self.entries.lock().expect("token cache poisoned");
            if let Some(entry) = entries.get(&key) {
                if Instant::now() < entry.expires {
                    return entry.identity.clone().unwrap_or_default();
                }
            }
        }
        let identity = check(token.to_string()).await;
        let ttl = if identity.is_some() {
            self.positive_ttl
        } else {
            self.negative_ttl
        };
        let mut entries = self.entries.lock().expect("token cache poisoned");
        let now = Instant::now();
        // Sweep what has expired first: a cache that is only ever asked about
        // distinct tokens still bounds itself between evictions, rather than
        // growing until the cap alone is doing the work.
        entries.retain(|_, cached| cached.expires > now);
        // The sweep is not a guarantee -- everything still live counts
        // against the cap -- so evict the entries soonest to expire anyway
        // until there is room. They are the closest to being swept on their
        // own, so removing them loses the least useful cached answer.
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
            key,
            CachedToken {
                identity: identity.clone(),
                expires: now + ttl,
            },
        );
        identity.unwrap_or_default()
    }
}

/// How long a pending sign-in from a terminal lives before it is forgotten.
/// Long enough to walk to a browser, short enough that a code someone read
/// over your shoulder is worthless by the time they type it.
pub const DEVICE_CODE_MAX_AGE: Duration = Duration::from_secs(10 * 60);

/// How long the token the flow hands out lives. Longer than a browser session,
/// because a terminal is not somewhere anybody wants to sign in weekly, and
/// still short enough that a forgotten laptop stops publishing eventually.
pub const DEVICE_TOKEN_MAX_AGE: Duration = Duration::from_secs(90 * 24 * 3600);

/// What the CLI is told to wait between polls. The whole exchange is with this
/// deployment and costs it a hash lookup, so there is no rate limit to respect
/// beyond not spinning.
pub const DEVICE_POLL_INTERVAL: u64 = 2;

/// A `kmd_` bearer is this deployment's own token: the session payload, signed
/// with the session key, verified locally with no network and no cache.
pub const DEVICE_TOKEN_PREFIX: &str = "kmd_";

/// The alphabet a user code is read aloud and typed from. No `O` or `0`, no
/// `I`, `1` or `L`: the code travels from a terminal to a browser through a
/// person's eyes, and those are the pairs that make that fail.
pub(super) const USER_CODE_ALPHABET: &[u8] = b"ABCDEFGHJKMNPQRSTUVWXYZ23456789";

pub(super) const USER_CODE_LENGTH: usize = 8;

/// How many sign-ins may be pending at once. The route needs no account, so
/// without a ceiling it is a way to make the server hold memory for nothing;
/// with one, the worst it can do is make its own next request wait ten
/// minutes.
pub const DEVICE_CODES_MAX: usize = 1000;

/// Eight characters, about forty bits. Guessing one inside ten minutes is not
/// a practical attack, and the worst it could do is put the guesser's own
/// identity on a stranger's terminal, which the stranger reads in `signed in
/// as`.
pub fn user_code() -> String {
    random_bytes(USER_CODE_LENGTH)
        .iter()
        .map(|byte| USER_CODE_ALPHABET[*byte as usize % USER_CODE_ALPHABET.len()] as char)
        .collect()
}

/// 128 random bits, which is what the terminal holds and never shows.
pub fn device_code() -> String {
    hex::encode(random_bytes(16))
}

pub(super) fn digest_of(value: &str) -> String {
    hex::encode(Sha256::digest(value.as_bytes()))
}

/// One terminal waiting to be signed in. The device code is kept as a digest,
/// the way a bearer token is anywhere else here: the table is the only place
/// it could leak from, and a digest is enough to recognise the holder.
pub(super) struct Pending {
    pub(super) device_digest: String,
    pub(super) created: i64,
    pub(super) approved: Option<Identity>,
}

/// The pending sign-ins, keyed by the user code the person types. It lives in
/// memory and nowhere else: a restart forgets every pending code, and a
/// `login` that was mid-flight is told to start again, which is the only harm.
pub struct PendingCodes {
    pub(super) entries: Mutex<HashMap<String, Pending>>,
    /// Seconds, so a test can shorten it through a shared reference rather
    /// than waiting out the real ten minutes.
    pub(super) max_age: std::sync::atomic::AtomicU64,
}

/// What a poll for the token learns.
pub enum DeviceOutcome {
    Pending,
    Expired,
    Approved(Identity),
}

impl Default for PendingCodes {
    fn default() -> Self {
        PendingCodes::new()
    }
}

impl PendingCodes {
    pub fn new() -> PendingCodes {
        PendingCodes {
            entries: Mutex::new(HashMap::new()),
            max_age: std::sync::atomic::AtomicU64::new(DEVICE_CODE_MAX_AGE.as_secs()),
        }
    }

    pub fn max_age(&self) -> i64 {
        self.max_age.load(std::sync::atomic::Ordering::Relaxed) as i64
    }

    #[allow(dead_code)] // only the tests shorten it; the server runs on the real ten minutes
    pub fn set_max_age(&self, seconds: u64) {
        self.max_age
            .store(seconds, std::sync::atomic::Ordering::Relaxed);
    }

    /// Starts a flow, or refuses when the table is full. Every entry that has
    /// aged out is dropped first, here and on every other access, so the table
    /// cannot grow past what ten minutes of real sign-ins put in it.
    pub fn start(&self) -> Option<(String, String)> {
        let mut entries = self.entries.lock().expect("pending codes poisoned");
        self.sweep(&mut entries);
        if entries.len() >= DEVICE_CODES_MAX {
            return None;
        }
        let user = user_code();
        let device = device_code();
        entries.insert(
            user.clone(),
            Pending {
                device_digest: digest_of(&device),
                created: now_unix(),
                approved: None,
            },
        );
        Some((device, user))
    }

    /// Binds an identity to a pending code. False when the code is unknown or
    /// has expired, which the caller answers 404 to.
    pub fn approve(&self, user: &str, who: &Identity) -> bool {
        let mut entries = self.entries.lock().expect("pending codes poisoned");
        self.sweep(&mut entries);
        match entries.get_mut(&normalized(user)) {
            Some(entry) => {
                entry.approved = Some(who.clone());
                true
            }
            None => false,
        }
    }

    /// What the terminal's poll gets. An approved entry is removed as it is
    /// read, so the token is handed out exactly once and a second poll --
    /// including one from somebody who found the device code afterwards --
    /// learns nothing.
    pub fn claim(&self, device: &str) -> DeviceOutcome {
        let mut entries = self.entries.lock().expect("pending codes poisoned");
        self.sweep(&mut entries);
        let digest = digest_of(device);
        let Some(user) = entries
            .iter()
            .find(|(_, entry)| entry.device_digest == digest)
            .map(|(user, _)| user.clone())
        else {
            // Unknown here means expired, forgotten across a restart, or
            // already claimed. The terminal can do the same thing about all
            // three, so they get the same answer.
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

    /// How many are waiting, for the tests and nothing else.
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
