//! Signing in from a terminal, and where the sign-in is kept: the token cache
//! under the config home, the device flow that fills it, and `logout`.

use super::*;

/// The config directory to read and write under, following XDG. This is the
/// only place any of the token helpers below touches the environment, so
/// everything else can be pure and tested by handing it a base directory
/// directly rather than mutating `$HOME` or `$XDG_CONFIG_HOME` for the whole
/// process.
pub(crate) fn config_home() -> PathBuf {
    match std::env::var("XDG_CONFIG_HOME") {
        Ok(base) if !base.is_empty() => PathBuf::from(base),
        _ => {
            let home = std::env::var("HOME")
                .ok()
                .filter(|h| !h.is_empty())
                .unwrap_or_else(|| die("no home directory to store the token in"));
            Path::new(&home).join(".config")
        }
    }
}

/// Where every komodoc file lives under the config directory.
pub(super) fn komodoc_dir(base: &Path) -> PathBuf {
    base.join("komodoc")
}

/// The single unscoped file `login` used to write before tokens were cached
/// per deployment. It is honoured only for the default server -- see
/// `stored_token_at` -- and removed once its token has been migrated into the
/// scoped cache, so it either holds the default server's token or does not
/// exist.
pub(crate) fn legacy_token_path(base: &Path) -> PathBuf {
    komodoc_dir(base).join("token")
}

/// One JSON object mapping a normalized server origin to the bearer token
/// `login` received from it. Scoped by origin, not by the literal `--server`
/// string, so `https://x.example` and `https://x.example/` share a cache
/// entry and a request never carries one deployment's token to another.
pub(crate) fn tokens_path(base: &Path) -> PathBuf {
    komodoc_dir(base).join("tokens.json")
}

/// All cached tokens, keyed by origin. A missing or unreadable file is the
/// same as no tokens cached yet, which is not worth failing a command over.
pub(super) fn load_tokens(base: &Path) -> std::collections::HashMap<String, String> {
    std::fs::read_to_string(tokens_path(base))
        .ok()
        .and_then(|raw| serde_json::from_str(&raw).ok())
        .unwrap_or_default()
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

/// The token cached for one server's origin, or "" if there is none. The
/// legacy unscoped file is consulted only when `server`'s origin is the
/// default server it predates -- it is never forwarded to a different
/// deployment, which is the bug this replaces.
pub(crate) fn stored_token_at(base: &Path, server: &str, default_server: &str) -> String {
    let origin = origin_of(server);
    if let Some(token) = load_tokens(base).get(&origin) {
        let trimmed = token.trim();
        if !trimmed.is_empty() {
            return trimmed.to_string();
        }
    }
    if origin == origin_of(default_server) {
        if let Ok(raw) = std::fs::read_to_string(legacy_token_path(base)) {
            let trimmed = raw.trim().to_string();
            if !trimmed.is_empty() {
                return trimmed;
            }
        }
    }
    String::new()
}

/// Caches `token` under `server`'s origin. When that origin is the default
/// server, the legacy unscoped file -- the only thing that could have been
/// caching a token for it before -- is migrated away: its content, if any, is
/// now superseded by this scoped entry, and leaving it in place would let a
/// stale copy resurface if the scoped cache were ever cleared.
pub(crate) fn store_token_at(
    base: &Path,
    server: &str,
    default_server: &str,
    token: &str,
) -> Result<(), String> {
    let origin = origin_of(server);
    let mut tokens = load_tokens(base);
    tokens.insert(origin.clone(), token.to_string());
    save_tokens(base, &tokens)?;
    if origin == origin_of(default_server) {
        let legacy = legacy_token_path(base);
        if legacy.exists() {
            std::fs::remove_file(&legacy)
                .map_err(|err| format!("could not remove {}: {err}", legacy.display()))?;
        }
    }
    Ok(())
}

/// The token to send to `server`: `$KOMODOC_TOKEN` if it is set, or whatever
/// `komodoc login` cached for that server's own origin.
///
/// `KOMODOC_TOKEN` is explicit and is sent to whichever server was selected,
/// wherever that is -- it is a bearer a shell script hands the CLI on
/// purpose, and there is nothing here to scope it against. The cache, by
/// contrast, is scoped to the server's origin precisely so that signing in to
/// one deployment never sends its token to another. A `KOMODOC_TOKEN` holding
/// a GitHub token still works: the server tells the two apart by the `kmd_`
/// prefix and verifies each its own way.
pub fn stored_token_for(server: &str) -> String {
    let env_token = std::env::var("KOMODOC_TOKEN").ok();
    stored_token_with(
        &config_home(),
        server,
        &default_server(),
        env_token.as_deref(),
    )
}

/// The pure core of `stored_token_for`: everything above it does is read the
/// environment and the config directory, which is factored out here so the
/// precedence between an explicit `KOMODOC_TOKEN` and the scoped cache can be
/// tested by passing values in, rather than by mutating the process
/// environment a test binary's threads share.
pub(crate) fn stored_token_with(
    base: &Path,
    server: &str,
    default_server: &str,
    env_token: Option<&str>,
) -> String {
    if let Some(token) = env_token {
        if !token.trim().is_empty() {
            return token.trim().to_string();
        }
    }
    stored_token_at(base, server, default_server)
}

/// What every command that writes needs.
pub fn require_token_for(server: &str) -> String {
    let token = stored_token_for(server);
    if token.is_empty() {
        die("not signed in. Run:\n    komodoc login");
    }
    token
}

pub async fn login(server_flag: String) {
    let server = server_from(&server_flag);
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

    let base = config_home();
    store_token_at(&base, &server, &default_server(), &token).unwrap_or_else(|err| die(err));
    println!("signed in as {who}");
    eprintln!("  token stored in {}", tokens_path(&base).display());
}

/// Writes bytes to a path only its owner can read, from the moment the file
/// is created -- there is no window where a broader mode briefly applies --
/// and fixes the permissions of a file that already existed under a looser
/// one. A bearer token's file permissions are the whole of its protection at
/// rest, so a failure to set them is reported rather than swallowed.
pub(super) fn write_private_file(path: &Path, bytes: &[u8]) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|err| format!("could not create {}: {err}", parent.display()))?;
    }
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options
        .open(path)
        .map_err(|err| format!("could not create {}: {err}", path.display()))?;
    use std::io::Write;
    file.write_all(bytes)
        .map_err(|err| format!("could not write {}: {err}", path.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        file.set_permissions(std::fs::Permissions::from_mode(0o600))
            .map_err(|err| format!("could not set permissions on {}: {err}", path.display()))?;
    }
    Ok(())
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
    let base = config_home();
    let cleared_scoped = std::fs::remove_file(tokens_path(&base)).is_ok();
    let legacy = legacy_token_path(&base);
    let cleared_legacy = match std::fs::remove_file(&legacy) {
        Ok(()) => true,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => false,
        Err(err) => die(format!("could not remove {}: {err}", legacy.display())),
    };
    if cleared_scoped || cleared_legacy {
        println!("signed out");
    } else {
        println!("not signed in");
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
