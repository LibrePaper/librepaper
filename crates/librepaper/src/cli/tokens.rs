//! Signing in from a terminal, and where the sign-in is kept: the token cache
//! under the state home, the device flow that fills it, and `logout`.

use super::*;
use crate::local::credentials::{
    librepaper_dir, lock_tokens, stored_token_at, store_token_at, tokens_path,
};
#[cfg(test)]
use crate::local::credentials::read_tokens;
pub(crate) use crate::local::credentials::write_private_file;
pub use crate::local::credentials::write_token;
pub(crate) use crate::local::paths::state_home_or_die as state_home;

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
