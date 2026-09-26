//! Read-only credential resolution shared by CLI and headless automation.

use std::collections::HashMap;
use std::path::Path;

pub(crate) fn origin(server: &str) -> String {
    match url::Url::parse(server) {
        Ok(url) if url.host_str().is_some() => format!(
            "{}://{}:{}",
            url.scheme(),
            url.host_str().unwrap_or_default(),
            url.port_or_known_default().unwrap_or(0)
        ),
        _ => server.trim().trim_end_matches('/').to_lowercase(),
    }
}

fn cached(base: &Path, server: &str) -> String {
    let path = base.join("librepaper").join("tokens.json");
    std::fs::read(&path)
        .ok()
        .and_then(|raw| serde_json::from_slice::<HashMap<String, String>>(&raw).ok())
        .and_then(|tokens| tokens.get(&origin(server)).cloned())
        .map(|token| token.trim().to_owned())
        .filter(|token| !token.is_empty())
        .unwrap_or_default()
}

pub(crate) fn agent_token(
    base: &Path,
    server: &str,
    configured_server: Option<&str>,
    explicit: Option<&str>,
) -> String {
    let explicit = explicit.filter(|_| {
        configured_server.is_some_and(|configured| origin(server) == origin(configured))
    });
    explicit
        .map(str::trim)
        .filter(|token| !token.is_empty())
        .map(str::to_owned)
        .unwrap_or_else(|| cached(base, server))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store(base: &Path, server: &str, token: &str) {
        let directory = base.join("librepaper");
        std::fs::create_dir_all(&directory).unwrap();
        let values = HashMap::from([(origin(server), token.to_string())]);
        std::fs::write(
            directory.join("tokens.json"),
            serde_json::to_vec(&values).unwrap(),
        )
        .unwrap();
    }

    #[test]
    fn automation_environment_token_requires_configured_origin() {
        let base = tempfile::tempdir().unwrap();
        store(base.path(), "https://other.test", "other-cache");
        assert_eq!(
            agent_token(
                base.path(),
                "https://home.test:443",
                Some("https://home.test/"),
                Some("explicit")
            ),
            "explicit"
        );
        assert_eq!(
            agent_token(
                base.path(),
                "https://other.test",
                Some("https://home.test"),
                Some("explicit")
            ),
            "other-cache"
        );
        assert_eq!(
            agent_token(base.path(), "https://unknown.test", None, Some("explicit")),
            ""
        );
    }
}
