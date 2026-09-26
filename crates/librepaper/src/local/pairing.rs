//! Who is allowed to ask this machine to compile: the pairing store the
//! loopback service consults on every authenticated request.
//!
//! State lives under `<state_home>/librepaper/local/` -- a sibling of, not
//! inside, the deployment token cache `crate::cli` keeps under
//! `<state_home>/librepaper/` -- as two small JSON files:
//!
//! - `service.json`: what the running `librepaper local start` printed, so
//!   `status`/`doctor`/`disconnect` and a second `start` can find it without
//!   asking the service itself.
//! - `pairings.json`: one entry per `(origin, project)` a browser has
//!   connected, keyed `"<origin>|<project>"`. The token itself is never
//!   written to disk -- only its SHA-256 -- so reading this file back does
//!   not hand out live access.
//!
//! Every function here takes the config-home base directory as an explicit
//! argument rather than reading `$XDG_STATE_HOME` itself, so a test can hand
//! it a temporary directory and never race another test over process-wide
//! environment state. `crate::cli::state_home()` is what production passes.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine as _;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::auth::{now_unix, random_bytes};

/// How long a pairing token is good for once issued.
pub const TOKEN_TTL_SECONDS: i64 = 30 * 24 * 3600;

/// What `librepaper local start` writes about itself, so a later command in a
/// different process invocation -- `status`, `disconnect`, a second `start`
/// checking whether one is already live -- can find the running service.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct ServiceState {
    pub port: u16,
    /// The random per-start identifier also returned from `GET health`, so
    /// `status` can tell a stale `service.json` (the pid is gone, or belongs
    /// to something else now) from the service it describes.
    pub instance: String,
    /// The six-digit pairing code for this run. Rotates every `start`.
    pub code: String,
    pub pid: u32,
    pub started: i64,
}

/// One granted `(origin, project)` pairing. `token_sha256` is the only trace
/// of the token this file keeps -- the plaintext is shown to the caller of
/// `connect` exactly once and never written down.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Pairing {
    pub token_sha256: String,
    pub origin: String,
    pub project: String,
    pub created: i64,
    pub expires: i64,
    #[serde(default)]
    pub label: String,
}

#[derive(Clone)]
pub struct PairingStore {
    dir: PathBuf,
    /// The fixed code `librepaper local start --code` was given, if any.
    /// Takes priority over whatever `service.json` holds, so `expected_code`
    /// answers correctly even when a caller never went through
    /// `write_service` at all, as tests that fix the code do not.
    fixed_code: Option<String>,
}

impl PairingStore {
    /// `state_home` is an XDG state base, e.g. `crate::cli::state_home()`
    /// in production or a temporary directory standing in for
    /// `$XDG_STATE_HOME` in a test. `fixed_code` is the resolved `--code`
    /// value, flag or its matching environment variable, when `librepaper
    /// local start` was given one; pass `None` everywhere else.
    pub fn new(state_home: &Path, fixed_code: Option<String>) -> Self {
        PairingStore {
            dir: state_home.join("librepaper").join("local"),
            fixed_code: fixed_code.filter(|c| !c.trim().is_empty()),
        }
    }

    fn service_path(&self) -> PathBuf {
        self.dir.join("service.json")
    }

    fn pairings_path(&self) -> PathBuf {
        self.dir.join("pairings.json")
    }

    pub fn write_service(&self, state: &ServiceState) -> std::io::Result<()> {
        write_private_json(&self.service_path(), state)
    }

    pub fn read_service(&self) -> Option<ServiceState> {
        read_json(&self.service_path())
    }

    pub fn remove_service(&self) {
        let _ = std::fs::remove_file(self.service_path());
    }

    /// The code a `connect` must present: the fixed code this store was
    /// constructed with, when there is one, else the code in
    /// `service.json`, else empty (nothing can connect to a service that
    /// has not called `write_service` yet).
    pub fn expected_code(&self) -> String {
        if let Some(code) = &self.fixed_code {
            return code.clone();
        }
        self.read_service().map(|s| s.code).unwrap_or_default()
    }

    fn load(&self) -> HashMap<String, Pairing> {
        read_json(&self.pairings_path()).unwrap_or_default()
    }

    fn save(&self, pairings: &HashMap<String, Pairing>) -> std::io::Result<()> {
        write_private_json(&self.pairings_path(), pairings)
    }

    /// Grants `project` under `origin` a fresh token, replacing any pairing
    /// already held for that exact key. Returns the plaintext token -- shown
    /// once -- and its expiry.
    pub fn issue(
        &self,
        origin: &str,
        project: &str,
        label: &str,
    ) -> std::io::Result<(String, i64)> {
        let origin = normalize_origin(origin);
        let token = random_token();
        let now = now_unix();
        let expires = now + TOKEN_TTL_SECONDS;
        let mut pairings = self.load();
        pairings.insert(
            key_of(&origin, project),
            Pairing {
                token_sha256: hash_token(&token),
                origin,
                project: project.to_string(),
                created: now,
                expires,
                label: label.to_string(),
            },
        );
        self.save(&pairings)?;
        Ok((token, expires))
    }

    /// The project `token` is live for under `origin`, or `None` when there
    /// is no such pairing, it expired, or the token does not match. The
    /// comparison is over the stored hash, not the plaintext, and takes the
    /// same time whether or not the bytes match.
    pub fn authenticate(&self, origin: &str, token: &str) -> Option<String> {
        if token.is_empty() {
            return None;
        }
        let origin = normalize_origin(origin);
        let hash = hash_token(token);
        let now = now_unix();
        self.load().into_values().find_map(|pairing| {
            let live = pairing.origin == origin && pairing.expires > now;
            let matches =
                crate::util::constant_time_eq(pairing.token_sha256.as_bytes(), hash.as_bytes());
            (live && matches).then_some(pairing.project)
        })
    }

    /// Whether `origin` holds any live pairing at all, regardless of token --
    /// what CORS asks before it echoes `Access-Control-Allow-Origin` on a
    /// route other than `health`/`connect`, where there is no bearer token on
    /// the request to check instead (a preflight carries none).
    pub fn has_live_pairing(&self, origin: &str) -> bool {
        let origin = normalize_origin(origin);
        let now = now_unix();
        self.load()
            .values()
            .any(|pairing| pairing.origin == origin && pairing.expires > now)
    }

    /// The `(origin, project)` pairs with a live pairing, for `status` and
    /// `start` to print.
    pub fn active_pairings(&self) -> Vec<(String, String)> {
        let now = now_unix();
        let mut pairings: Vec<_> = self
            .load()
            .into_values()
            .filter(|p| p.expires > now)
            .map(|p| (p.origin, p.project))
            .collect();
        pairings.sort();
        pairings
    }

    /// Revokes exactly one `(origin, project)` pairing -- what `POST
    /// disconnect` does to the token that authenticated it.
    pub fn revoke_one(&self, origin: &str, project: &str) -> bool {
        let origin = normalize_origin(origin);
        let mut pairings = self.load();
        let removed = pairings.remove(&key_of(&origin, project)).is_some();
        if removed {
            let _ = self.save(&pairings);
        }
        removed
    }

    /// Revokes every project under `origin`, or -- with `None` -- every
    /// pairing this store holds. What `librepaper local disconnect` does.
    /// Returns how many were removed.
    pub fn revoke(&self, origin: Option<&str>) -> usize {
        let mut pairings = self.load();
        let before = pairings.len();
        match origin {
            None => pairings.clear(),
            Some(origin) => {
                let origin = normalize_origin(origin);
                pairings.retain(|_, p| p.origin != origin);
            }
        }
        let removed = before - pairings.len();
        if removed > 0 {
            let _ = self.save(&pairings);
        }
        removed
    }
}

fn key_of(origin: &str, project: &str) -> String {
    format!("{origin}|{project}")
}

/// 32 random bytes, base64url without padding -- what a bearer token is.
pub fn random_token() -> String {
    URL_SAFE_NO_PAD.encode(random_bytes(32))
}

/// A fresh six-digit pairing code, as `librepaper local start` prints and
/// rotates on every run.
pub fn generate_code() -> String {
    use rand::Rng;
    let value: u32 = rand::rng().random_range(0..1_000_000);
    format!("{value:06}")
}

fn hash_token(token: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(token.as_bytes());
    hex::encode(hasher.finalize())
}

/// scheme + host + port, lowercase, with no path, query or trailing slash --
/// two spellings of the same browser origin must land on the same pairing.
/// A value that does not parse as a URL is lowercased and trimmed as-is
/// rather than rejected, so a caller sees a pairing miss, not a panic.
pub fn normalize_origin(raw: &str) -> String {
    match url::Url::parse(raw.trim()) {
        Ok(url) => {
            let scheme = url.scheme().to_ascii_lowercase();
            let host = url.host_str().unwrap_or("").to_ascii_lowercase();
            match url.port() {
                Some(port) => format!("{scheme}://{host}:{port}"),
                None => format!("{scheme}://{host}"),
            }
        }
        Err(_) => raw.trim().to_ascii_lowercase(),
    }
}

/// An origin as a browser or a `librepaper://` link would send it: an
/// http(s) scheme and a host, and nothing else -- no path but `/`, no query,
/// fragment or embedded credentials. Returns it normalised the way the
/// pairing store keys origins.
pub(crate) fn valid_origin(raw: &str) -> Option<String> {
    let parsed = url::Url::parse(raw.trim()).ok()?;
    if !matches!(parsed.scheme(), "http" | "https")
        || parsed.host_str().is_none()
        || parsed.path() != "/"
        || parsed.query().is_some()
        || parsed.fragment().is_some()
        || !parsed.username().is_empty()
        || parsed.password().is_some()
    {
        return None;
    }
    Some(normalize_origin(raw.trim()))
}

/// A project name: non-empty, bounded, and drawn from the characters a
/// document identifier needs.
pub(crate) fn valid_project(raw: &str) -> Option<String> {
    let value = raw.trim();
    let ok = !value.is_empty()
        && value.len() <= 256
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | ':'));
    ok.then(|| value.to_string())
}

/// A pairing request id: what the browser generates client side to track one
/// attempt across the consent page and the claim loop. The browser always
/// generates base64url, so this is the strict reading -- ASCII alphanumeric
/// plus `-` and `_` only, never `.`.
pub(crate) fn valid_request_id(raw: &str) -> Option<String> {
    let value = raw.trim();
    (value.len() >= 32
        && value.len() <= 128
        && value
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'-' | b'_')))
    .then(|| value.to_string())
}

/// A PKCE challenge: the lowercase hex SHA-256 digest of the verifier the
/// browser keeps to itself.
pub(crate) fn valid_challenge(raw: &str) -> Option<String> {
    let value = raw.trim().to_ascii_lowercase();
    (value.len() == 64 && value.bytes().all(|c| c.is_ascii_hexdigit())).then_some(value)
}

/// Where the consent result page, or a `librepaper://launch` link, sends the
/// browser back to when there is no opener to poll claim for it: an absolute
/// http(s) URL, bounded, without a fragment or embedded credentials, whose
/// normalised origin is exactly `origin`.
pub(crate) fn valid_return(raw: &str, origin: &str) -> Option<String> {
    let value = raw.trim();
    if value.is_empty() || value.len() > 2048 {
        return None;
    }
    let parsed = url::Url::parse(value).ok()?;
    if !matches!(parsed.scheme(), "http" | "https")
        || parsed.host_str().is_none()
        || parsed.fragment().is_some()
        || !parsed.username().is_empty()
        || parsed.password().is_some()
        || normalize_origin(value) != normalize_origin(origin)
    {
        return None;
    }
    Some(value.to_string())
}

/// The fragment a link handler, or the consent result page, hands back to a
/// page it cannot otherwise reach: the companion's real address and the
/// request id that proves the fragment came from an attempt the page itself
/// started.
pub(crate) fn return_fragment(return_to: &str, port: u16, request: &str) -> String {
    let address = format!("http://127.0.0.1:{port}/");
    let encoded_address: String =
        percent_encoding::utf8_percent_encode(&address, percent_encoding::NON_ALPHANUMERIC)
            .collect();
    let encoded_request: String =
        percent_encoding::utf8_percent_encode(request, percent_encoding::NON_ALPHANUMERIC)
            .collect();
    format!("{return_to}#librepaper-local={encoded_address}&librepaper-request={encoded_request}")
}

pub(crate) fn write_private_json<T: Serialize>(path: &Path, value: &T) -> std::io::Result<()> {
    let body = serde_json::to_vec_pretty(value).map_err(std::io::Error::other)?;
    crate::private_files::publish(path, &body, "local private JSON").map_err(std::io::Error::other)
}

pub(crate) fn read_json<T: for<'de> Deserialize<'de>>(path: &Path) -> Option<T> {
    let text = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&text).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> (tempfile::TempDir, PairingStore) {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = PairingStore::new(dir.path(), None);
        (dir, store)
    }

    #[test]
    fn issued_token_authenticates_and_is_stored_hashed() {
        let (_dir, store) = store();
        let (token, _expires) = store
            .issue("https://Example.com:443", "proj", "browser")
            .expect("issue");
        assert_eq!(
            store.authenticate("https://example.com:443", &token),
            Some("proj".to_string())
        );
        let raw = std::fs::read_to_string(store.pairings_path()).expect("read");
        assert!(!raw.contains(&token), "plaintext token leaked to disk");
    }

    #[test]
    fn wrong_token_or_origin_does_not_authenticate() {
        let (_dir, store) = store();
        let (token, _) = store.issue("https://a", "proj", "").expect("issue");
        assert_eq!(store.authenticate("https://a", "wrong"), None);
        assert_eq!(store.authenticate("https://b", &token), None);
    }

    #[test]
    fn revoke_one_leaves_other_projects_paired() {
        let (_dir, store) = store();
        let (t1, _) = store.issue("https://a", "p1", "").expect("issue");
        let (t2, _) = store.issue("https://a", "p2", "").expect("issue");
        assert!(store.revoke_one("https://a", "p1"));
        assert_eq!(store.authenticate("https://a", &t1), None);
        assert_eq!(store.authenticate("https://a", &t2), Some("p2".to_string()));
    }

    #[test]
    fn a_fixed_code_wins_over_service_json() {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = PairingStore::new(dir.path(), Some("123456".to_string()));
        store
            .write_service(&ServiceState {
                port: 0,
                instance: "test".to_string(),
                code: "000000".to_string(),
                pid: 1,
                started: 0,
            })
            .expect("write service.json");
        assert_eq!(store.expected_code(), "123456");
    }

    #[test]
    fn origin_normalisation_ignores_case_and_matches_port() {
        assert_eq!(
            normalize_origin("HTTPS://LibrePaper.Example:5173/"),
            normalize_origin("https://librepaper.example:5173")
        );
        assert_ne!(
            normalize_origin("https://librepaper.example"),
            normalize_origin("https://librepaper.example:5173")
        );
    }

    #[test]
    fn replacing_a_grant_invalidates_the_previous_token() {
        let (_dir, store) = store();
        let (old, _) = store
            .issue("https://example.test", "paper", "old")
            .expect("issue");
        let (new, _) = store
            .issue("https://example.test", "paper", "new")
            .expect("issue");
        assert_eq!(store.authenticate("https://example.test", &old), None);
        assert_eq!(
            store.authenticate("https://example.test", &new),
            Some("paper".into())
        );
    }

    #[test]
    fn valid_origin_rejects_anything_but_a_bare_http_s_origin() {
        assert_eq!(
            valid_origin("HTTPS://Papers.Example/"),
            Some(normalize_origin("https://papers.example"))
        );
        for rejected in [
            "ftp://papers.example/",
            "https://papers.example/doc",
            "https://papers.example/?q=1",
            "https://papers.example/#frag",
            "https://user:pass@papers.example/",
            "not a url",
        ] {
            assert!(valid_origin(rejected).is_none(), "accepted {rejected}");
        }
    }

    #[test]
    fn valid_project_is_bounded_and_charset_limited() {
        assert_eq!(
            valid_project("paper-1.review:2"),
            Some("paper-1.review:2".into())
        );
        assert!(valid_project("").is_none());
        assert!(valid_project(&"a".repeat(257)).is_none());
        assert!(valid_project("has space").is_none());
    }

    #[test]
    fn valid_request_id_is_strict_base64url_with_no_dot() {
        let id = "r".repeat(32);
        assert_eq!(valid_request_id(&id), Some(id.clone()));
        assert!(valid_request_id(&"r".repeat(31)).is_none());
        assert!(valid_request_id(&"r".repeat(129)).is_none());
        assert!(valid_request_id(&format!("{}.", "r".repeat(31))).is_none());
    }

    #[test]
    fn valid_challenge_requires_a_lowercase_hex_sha256_digest() {
        let digest = "a".repeat(64);
        assert_eq!(valid_challenge(&digest.to_ascii_uppercase()), Some(digest));
        assert!(valid_challenge(&"a".repeat(63)).is_none());
        assert!(valid_challenge(&"g".repeat(64)).is_none());
    }

    #[test]
    fn valid_return_requires_the_same_origin_as_the_pairing() {
        let origin = "https://paper.example".to_string();
        assert_eq!(
            valid_return("https://paper.example/document/1", &origin),
            Some("https://paper.example/document/1".to_string())
        );
        // A different origin, scheme or port is rejected even though the
        // rest of the URL is well formed.
        for rejected in [
            "https://evil.example/document/1",
            "http://paper.example/document/1",
            "https://paper.example:8443/document/1",
            "ftp://paper.example/document/1",
            "https://user:pass@paper.example/document/1",
            "https://paper.example/document/1#fragment",
            "not a url",
            "",
        ] {
            assert!(
                valid_return(rejected, &origin).is_none(),
                "accepted {rejected}"
            );
        }
    }

    #[test]
    fn valid_return_is_bounded_in_length() {
        let origin = "https://paper.example".to_string();
        let long = format!(
            "https://paper.example/{}",
            "a".repeat(2048 - "https://paper.example/".len() + 1)
        );
        assert!(long.len() > 2048);
        assert!(valid_return(&long, &origin).is_none());
    }

    #[test]
    fn return_fragment_carries_the_address_and_request_percent_encoded() {
        let fragment = return_fragment("https://paper.example/doc", 18763, "r".repeat(32).as_str());
        assert!(fragment.starts_with("https://paper.example/doc#librepaper-local="));
        assert!(fragment.contains("librepaper-request="));
        assert!(fragment.contains("http%3A%2F%2F127%2E0%2E0%2E1%3A18763%2F"));
    }

    #[test]
    fn expired_grants_and_wrong_origin_or_project_scope_are_rejected() {
        let (_dir, store) = store();
        let (token, _) = store
            .issue("https://example.test", "paper", "")
            .expect("issue");
        let mut grants = store.load();
        let key = key_of("https://example.test", "paper");
        grants.get_mut(&key).expect("grant").expires = now_unix() - 1;
        store.save(&grants).expect("save");
        assert_eq!(store.authenticate("https://example.test", &token), None);
        assert_eq!(store.authenticate("https://other.test", &token), None);
        // Authentication returns the token's project; callers must compare it
        // with the requested project before accepting a job or binding.
        let (token, _) = store
            .issue("https://example.test", "paper", "")
            .expect("issue");
        assert_eq!(
            store.authenticate("https://example.test", &token),
            Some("paper".into())
        );
        assert_ne!(
            store.authenticate("https://example.test", &token),
            Some("other".into())
        );
    }
}
