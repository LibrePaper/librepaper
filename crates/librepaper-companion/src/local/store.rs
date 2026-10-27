//! Small JSON state files, and the validators for the names that key them.
//!
//! State lives under `<state_home>/librepaper/local/`, a sibling of, not
//! inside, the deployment token cache `local::credentials` keeps under
//! `<state_home>/librepaper/`. `service.json` is what the running
//! `librepaper local start` printed about itself, so `status`, `endpoint` and a
//! second `start` can find it without asking the service.
//!
//! Every function here takes the state-home base directory as an explicit
//! argument rather than reading `$XDG_STATE_HOME` itself, so a test can hand
//! it a temporary directory and never race another test over process-wide
//! environment state. `crate::local::paths::state_home_or_die()` is what production passes.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// What `librepaper local start` writes about itself, so a later command in a
/// different process invocation -- `status`, `endpoint`, a second `start`
/// checking whether one is already live -- can find the running service.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct ServiceState {
    pub port: u16,
    /// The random per-start identifier also returned from `GET health`, so
    /// `status` can tell a stale `service.json` (the pid is gone, or belongs
    /// to something else now) from the service it describes.
    pub instance: String,
    pub pid: u32,
    pub started: i64,
}

/// Reads and writes `service.json` under a state home.
#[derive(Clone)]
pub struct ServiceFile {
    dir: PathBuf,
}

impl ServiceFile {
    /// `state_home` is an XDG state base, e.g. `crate::local::paths::state_home_or_die()`
    /// in production or a temporary directory standing in for
    /// `$XDG_STATE_HOME` in a test.
    pub fn new(state_home: &Path) -> Self {
        ServiceFile {
            dir: state_home.join("librepaper").join("local"),
        }
    }

    fn path(&self) -> PathBuf {
        self.dir.join("service.json")
    }

    pub fn write(&self, state: &ServiceState) -> std::io::Result<()> {
        write_private_json(&self.path(), state)
    }

    pub fn read(&self) -> Option<ServiceState> {
        read_json(&self.path())
    }

    pub fn remove(&self) {
        let _ = std::fs::remove_file(self.path());
    }
}

/// scheme + host + port, lowercase, with no path, query or trailing slash --
/// two spellings of the same browser origin must land on the same key.
/// A value that does not parse as a URL is lowercased and trimmed as-is
/// rather than rejected, so a caller sees a miss, not a panic.
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

/// An origin as the desktop shell sends it: an http(s) scheme and a host,
/// and nothing else -- no path but `/`, no query, fragment or embedded
/// credentials. Returns it normalised the way origins are keyed.
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

/// Whether `url` names this computer: `localhost` or a loopback address. Plain
/// http is acceptable to a server or page only when this holds.
pub(crate) fn is_loopback(url: &url::Url) -> bool {
    url.host().is_some_and(|host| match host {
        url::Host::Domain(domain) => domain == "localhost",
        url::Host::Ipv4(address) => address.is_loopback(),
        url::Host::Ipv6(address) => address.is_loopback(),
    })
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

pub(crate) fn write_private_json<T: Serialize>(path: &Path, value: &T) -> std::io::Result<()> {
    let body = serde_json::to_vec_pretty(value).map_err(std::io::Error::other)?;
    librepaper_base::private_files::publish(path, &body, "local private JSON")
        .map_err(std::io::Error::other)
}

pub(crate) fn read_json<T: for<'de> Deserialize<'de>>(path: &Path) -> Option<T> {
    let text = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&text).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn service_file_round_trips_and_removes() {
        let dir = tempfile::tempdir().expect("tempdir");
        let file = ServiceFile::new(dir.path());
        assert!(file.read().is_none());
        let state = ServiceState {
            port: 8763,
            instance: "abc".into(),
            pid: 7,
            started: 1,
        };
        file.write(&state).expect("write");
        assert_eq!(file.read(), Some(state));
        file.remove();
        assert!(file.read().is_none());
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
}
