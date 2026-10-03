//! Read-only credential resolution shared by CLI and headless automation.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

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

/// Where every librepaper file lives under the state directory.
pub fn librepaper_dir(base: &Path) -> PathBuf {
    base.join("librepaper")
}

/// One JSON object mapping a normalized server origin to the bearer token
/// `login` received from it. Scoped by origin, not by the literal `--server`
/// string, so `https://x.example` and `https://x.example/` share a cache
/// entry and a request never carries one deployment's token to another.
pub fn tokens_path(base: &Path) -> PathBuf {
    librepaper_dir(base).join("tokens.json")
}

/// All cached tokens, keyed by origin. A missing or unreadable file is the
/// same as no tokens cached yet, which is not worth failing a command over.
pub(crate) fn load_tokens(base: &Path) -> std::collections::HashMap<String, String> {
    read_tokens(base).unwrap_or_default()
}

pub fn read_tokens(base: &Path) -> Result<std::collections::HashMap<String, String>, String> {
    let path = tokens_path(base);
    let raw = match std::fs::read_to_string(&path) {
        Ok(raw) => raw,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(Default::default()),
        Err(err) => return Err(format!("could not read {}: {err}", path.display())),
    };
    serde_json::from_str(&raw)
        .map_err(|err| format!("invalid token cache {}: {err}", path.display()))
}

// Never unlink this lock: replacing its inode would let two processes lock
// different files. The operating system releases the lock even after a crash.
pub fn lock_tokens(base: &Path) -> Result<std::fs::File, String> {
    let directory = librepaper_dir(base);
    std::fs::create_dir_all(&directory)
        .map_err(|err| format!("could not create {}: {err}", directory.display()))?;
    let path = directory.join("tokens.lock");
    let mut options = std::fs::OpenOptions::new();
    options.read(true).write(true).create(true).truncate(false);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let file = options
        .open(&path)
        .map_err(|err| format!("could not open {}: {err}", path.display()))?;
    fs2::FileExt::lock_exclusive(&file)
        .map_err(|err| format!("could not lock {}: {err}", path.display()))?;
    Ok(file)
}

pub(crate) fn save_tokens(
    base: &Path,
    tokens: &std::collections::HashMap<String, String>,
) -> Result<(), String> {
    let path = tokens_path(base);
    let body = serde_json::to_string_pretty(tokens)
        .map_err(|err| format!("could not encode {}: {err}", path.display()))?;
    // `write_token` is the general "write this text where nobody else can
    // read it" primitive, not only the one `login` used to use for a lone
    // bearer string; a trailing newline on a JSON file is harmless.
    write_token(&path, &body)
}

/// The token cached for one server's origin, or "" if there is none.
pub fn stored_token_at(base: &Path, server: &str) -> String {
    let origin = origin(server);
    load_tokens(base)
        .get(&origin)
        .map(|token| token.trim().to_string())
        .filter(|token| !token.is_empty())
        .unwrap_or_default()
}

/// Caches `token` under `server`'s origin.
pub fn store_token_at(base: &Path, server: &str, token: &str) -> Result<(), String> {
    let _lock = lock_tokens(base)?;
    let origin = origin(server);
    let mut tokens = read_tokens(base)?;
    tokens.insert(origin, token.to_string());
    save_tokens(base, &tokens)
}

/// Replaces `path` with `bytes`, readable by nobody else.
///
/// The private-replacement mechanics -- create the temporary already
/// private, sync it, close it before the rename, sync the directory, take
/// the temporary away on failure -- are [`librepaper_base::private_files::publish`]'s,
/// shared with the assistant journal so the two cannot drift. Caller
/// locking stays here: `save_tokens` holds the tokens lock across a
/// read-modify-write, which no single replacement can provide.
pub fn write_private_file(path: &Path, bytes: &[u8]) -> Result<(), String> {
    librepaper_base::private_files::publish(path, bytes, &path.display().to_string())
}

/// Writes the token where the next command will look for it, readable by
/// nobody else. Kept as a thin wrapper over `write_private_file` because
/// tests write a token to an arbitrary path directly, without going through
/// `login`'s scoped cache.
pub fn write_token(path: &Path, token: &str) -> Result<(), String> {
    write_private_file(path, format!("{token}\n").as_bytes())
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
