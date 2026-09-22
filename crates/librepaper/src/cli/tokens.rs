//! Signing in from a terminal, and where the sign-in is kept: the token cache
//! under the state home, the device flow that fills it, and `logout`.

use super::*;

/// The state directory to read and write under, following XDG. What lives
/// here is written by the program, not by a person: the token cache, and the
/// local service's pairings and tool cache. That is state, not
/// configuration, so it goes under `$XDG_STATE_HOME` (or `~/.local/state`),
/// not the config home. This is the only place any of the token helpers below
/// touches the environment, so everything else can be pure and tested by
/// handing it a base directory directly rather than mutating `$HOME` or
/// `$XDG_STATE_HOME` for the whole process.
pub(crate) fn state_home() -> PathBuf {
    crate::local::paths::state_home().unwrap_or_else(|error| die(&error))
}

/// Where every librepaper file lives under the state directory.
pub(super) fn librepaper_dir(base: &Path) -> PathBuf {
    base.join("librepaper")
}

/// One JSON object mapping a normalized server origin to the bearer token
/// `login` received from it. Scoped by origin, not by the literal `--server`
/// string, so `https://x.example` and `https://x.example/` share a cache
/// entry and a request never carries one deployment's token to another.
pub(crate) fn tokens_path(base: &Path) -> PathBuf {
    librepaper_dir(base).join("tokens.json")
}

/// All cached tokens, keyed by origin. A missing or unreadable file is the
/// same as no tokens cached yet, which is not worth failing a command over.
pub(super) fn load_tokens(base: &Path) -> std::collections::HashMap<String, String> {
    read_tokens(base).unwrap_or_default()
}

fn read_tokens(base: &Path) -> Result<std::collections::HashMap<String, String>, String> {
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
fn lock_tokens(base: &Path) -> Result<std::fs::File, String> {
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

pub(super) fn save_tokens(
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
pub(crate) fn stored_token_at(base: &Path, server: &str) -> String {
    let origin = crate::local::credentials::origin(server);
    load_tokens(base)
        .get(&origin)
        .map(|token| token.trim().to_string())
        .filter(|token| !token.is_empty())
        .unwrap_or_default()
}

/// Caches `token` under `server`'s origin.
pub(crate) fn store_token_at(base: &Path, server: &str, token: &str) -> Result<(), String> {
    let _lock = lock_tokens(base)?;
    let origin = crate::local::credentials::origin(server);
    let mut tokens = read_tokens(base)?;
    tokens.insert(origin, token.to_string());
    save_tokens(base, &tokens)
}

/// The token to send to `server`: an explicit `--token` (or the
/// `$LIBREPAPER_TOKEN` clap merges into it) if one was given, or whatever
/// `librepaper login` cached for that server's own origin.
///
/// An explicit token is sent to whichever server was selected, wherever that
/// is -- it is a bearer given to the CLI on purpose, and there is nothing
/// here to scope it against. The cache, by contrast, is scoped to the
/// server's origin precisely so that signing in to one deployment never sends
/// its token to another. An explicit token holding a GitHub token still
/// works: the server tells the two apart by the `lp_` prefix and verifies
/// each its own way.
pub fn stored_token_for(server: &str, token: Option<&str>) -> String {
    stored_token_with(&state_home(), server, token)
}

/// The pure core of `stored_token_for`: everything above it does is read the
/// state directory, which is factored out here so the precedence between an
/// explicit token and the scoped cache can be tested by passing values in,
/// rather than by mutating the process environment a test binary's threads
/// share.
pub(crate) fn stored_token_with(base: &Path, server: &str, token: Option<&str>) -> String {
    if let Some(token) = token {
        if !token.trim().is_empty() {
            return token.trim().to_string();
        }
    }
    stored_token_at(base, server)
}

/// What every command that writes needs.
pub fn require_token_for(server: &str, token: Option<&str>) -> String {
    let resolved = stored_token_for(server, token);
    if resolved.is_empty() {
        die("not signed in. Run:\n    librepaper login");
    }
    resolved
}

pub async fn login(server: Option<String>) {
    let server = server_or_die(server);
    let code = request_device_code(&server)
        .await
        .unwrap_or_else(|err| die(format!("could not start the sign-in: {err}")));
    eprintln!(
        "\n  Open {}\n  and enter the code:  {}\n",
        code.verification_url, code.user_code
    );
    eprint!("  waiting for you to approve it");

    let token = poll_for_token(&server, &code).await;
    eprintln!();
    let token = token.unwrap_or_else(|err| die(err));

    // Who the token says you are. It is this deployment's own token, so the
    // deployment is the only thing that can answer, and it costs one call.
    let who =
        match get_with_token(&format!("{server}/api/me"), &token, Duration::from_secs(30)).await {
            Ok((200, payload)) => text(&payload, "name"),
            _ => String::new(),
        };

    let base = state_home();
    store_token_at(&base, &server, &token).unwrap_or_else(|err| die(err));
    if who.is_empty() {
        println!("signed in");
    } else {
        println!("signed in as {who}");
    }
    eprintln!("  token stored in {}", tokens_path(&base).display());
}

/// Creates a private file without ever opening an existing path.  Export
/// staging files use this instead of truncating a predictable sibling: two
/// concurrent exports must not share a temporary file, and an interrupted
/// export must not destroy an unrelated file with the same name.
pub(super) fn create_private_file_exclusive(path: &Path) -> Result<std::fs::File, String> {
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    std::fs::create_dir_all(parent)
        .map_err(|err| format!("could not create {}: {err}", parent.display()))?;
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options
        .open(path)
        .map_err(|err| format!("could not create {}: {err}", path.display()))
}

/// Replaces `path` with `bytes`, readable by nobody else.
///
/// The private-replacement mechanics -- create the temporary already
/// private, sync it, close it before the rename, sync the directory, take
/// the temporary away on failure -- are [`crate::private_files::publish`]'s,
/// shared with the assistant journal so the two cannot drift. Caller
/// locking stays here: `save_tokens` holds the tokens lock across a
/// read-modify-write, which no single replacement can provide.
pub(super) fn write_private_file(path: &Path, bytes: &[u8]) -> Result<(), String> {
    crate::private_files::publish(path, bytes, &path.display().to_string())
}

/// Writes the token where the next command will look for it, readable by
/// nobody else. Kept as a thin wrapper over `write_private_file` because
/// tests write a token to an arbitrary path directly, without going through
/// `login`'s scoped cache.
pub fn write_token(path: &Path, token: &str) -> Result<(), String> {
    write_private_file(path, format!("{token}\n").as_bytes())
}

/// Signs out of every cached deployment at once: `logout` takes no `--server`
/// of its own, so there is no single origin to clear selectively.
pub fn logout() {
    let base = state_home();
    if logout_at(&base).unwrap_or_else(|err| die(err)) {
        println!("signed out");
    } else {
        println!("not signed in");
    }
}

fn logout_at(base: &Path) -> Result<bool, String> {
    if !librepaper_dir(base).exists() {
        return Ok(false);
    }
    let _lock = lock_tokens(base)?;
    let path = tokens_path(base);
    match std::fs::remove_file(&path) {
        Ok(()) => Ok(true),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(err) => Err(format!("could not remove {}: {err}", path.display())),
    }
}

pub struct DeviceCode {
    pub device_code: String,
    pub user_code: String,
    pub verification_url: String,
    pub expires_in: u64,
    pub interval: u64,
}

pub async fn request_device_code(server: &str) -> Result<DeviceCode, String> {
    // No bearer: the terminal has no token yet, which is the whole reason it
    // is asking.
    let (status, payload) = post_json(
        &format!("{server}/api/auth/device"),
        &json!({}),
        "",
        Duration::from_secs(30),
    )
    .await?;
    let device_code = text(&payload, "device_code");
    if device_code.is_empty() {
        return Err(format!(
            "{server} returned {status}: {}",
            detail_of(&payload)
        ));
    }
    Ok(DeviceCode {
        device_code,
        user_code: text(&payload, "user_code"),
        verification_url: text(&payload, "verification_url"),
        expires_in: payload
            .get("expires_in")
            .and_then(Value::as_u64)
            .unwrap_or(600),
        interval: payload
            .get("interval")
            .and_then(Value::as_u64)
            .filter(|i| *i > 0)
            .unwrap_or(5),
    })
}

/// Waits for the code to be approved, at the interval the server asks for and
/// no faster. The deadline is the server's own expiry, so a code the server
/// has already forgotten is not polled for after it says so.
pub async fn poll_for_token(server: &str, code: &DeviceCode) -> Result<String, String> {
    let deadline = std::time::Instant::now() + Duration::from_secs(code.expires_in.max(60));
    let interval = Duration::from_secs(code.interval);
    while std::time::Instant::now() < deadline {
        tokio::time::sleep(interval).await;
        eprint!(".");
        let Ok((_, reply)) = post_json(
            &format!("{server}/api/auth/device/token"),
            &json!({"device_code": code.device_code}),
            "",
            Duration::from_secs(30),
        )
        .await
        else {
            continue;
        };
        let token = text(&reply, "token");
        if !token.is_empty() {
            return Ok(token);
        }
        match text(&reply, "error").as_str() {
            "authorization_pending" | "" => {}
            "expired_token" => return Err("the code expired before it was approved".into()),
            other => return Err(format!("the server said: {other}")),
        }
    }
    Err("the code expired before it was approved".into())
}

#[cfg(test)]
mod cache_tests {
    use super::*;

    #[test]
    fn corrupt_cache_is_preserved_when_login_writes() {
        let base = tempfile::tempdir().unwrap();
        write_token(&tokens_path(base.path()), "{broken").unwrap();
        assert!(store_token_at(base.path(), "https://new.test", "new").is_err());
        assert_eq!(
            std::fs::read_to_string(tokens_path(base.path())).unwrap(),
            "{broken\n"
        );
    }

    #[test]
    fn concurrent_logins_keep_every_origin() {
        let base = tempfile::tempdir().unwrap();
        std::thread::scope(|scope| {
            for n in 0..8 {
                let base = base.path();
                scope.spawn(move || {
                    store_token_at(base, &format!("https://host{n}.test"), "token").unwrap()
                });
            }
        });
        assert_eq!(read_tokens(base.path()).unwrap().len(), 8);
    }

    #[test]
    fn logout_reports_failure_to_remove_a_cache() {
        let base = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tokens_path(base.path())).unwrap();
        assert!(logout_at(base.path()).is_err());
        assert!(tokens_path(base.path()).exists());
    }

    #[cfg(unix)]
    #[test]
    fn replacing_a_loose_file_does_not_write_secrets_into_its_inode() {
        use std::io::Read;
        use std::os::unix::fs::PermissionsExt;
        let base = tempfile::tempdir().unwrap();
        let path = base.path().join("token");
        std::fs::write(&path, "old").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        let mut previously_opened = std::fs::File::open(&path).unwrap();
        write_private_file(&path, b"new-private-token").unwrap();
        let mut old_contents = String::new();
        previously_opened.read_to_string(&mut old_contents).unwrap();
        assert_eq!(old_contents, "old");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "new-private-token");
        assert_eq!(
            std::fs::metadata(path).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
}
