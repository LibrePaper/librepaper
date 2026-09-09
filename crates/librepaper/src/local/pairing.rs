//! Who is allowed to ask this machine to compile: the pairing store the
//! loopback service consults on every authenticated request.
//!
//! State lives under `<config_home>/librepaper/local/` -- a sibling of, not
//! inside, the deployment token cache `crate::cli` keeps under
//! `<config_home>/librepaper/` -- as two small JSON files:
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
//! argument rather than reading `$XDG_CONFIG_HOME` itself, so a test can hand
//! it a temporary directory and never race another test over process-wide
//! environment state. `crate::cli::config_home()` is what production passes.

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
}

impl PairingStore {
    /// `config_home` is an XDG config base, e.g. `crate::cli::config_home()`
    /// in production or a temporary directory standing in for
    /// `$XDG_CONFIG_HOME` in a test.
    pub fn new(config_home: &Path) -> Self {
        PairingStore {
            dir: config_home.join("librepaper").join("local"),
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

    /// The code a `connect` must present: `$LIBREPAPER_LOCAL_CODE` when set --
    /// so a test never has to read `service.json` back to know it -- else
    /// the code in `service.json`, else empty (nothing can connect to a
    /// service that has not called `write_service` yet).
    pub fn expected_code(&self) -> String {
        if let Ok(code) = std::env::var("LIBREPAPER_LOCAL_CODE") {
            if !code.trim().is_empty() {
                return code.trim().to_string();
            }
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
            let matches = constant_time_eq(pairing.token_sha256.as_bytes(), hash.as_bytes());
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

/// Compares two equal-length ASCII strings (hex digests here) without
/// branching on the first differing byte, so a wrong guess and a near-miss
/// take the same time to be told apart from a live token.
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
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

fn write_private_json<T: Serialize>(path: &Path, value: &T) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let body = serde_json::to_string_pretty(value).map_err(std::io::Error::other)?;
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path)?;
    use std::io::Write;
    file.write_all(body.as_bytes())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}

fn read_json<T: for<'de> Deserialize<'de>>(path: &Path) -> Option<T> {
    let text = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&text).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> (tempfile::TempDir, PairingStore) {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = PairingStore::new(dir.path());
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
}
