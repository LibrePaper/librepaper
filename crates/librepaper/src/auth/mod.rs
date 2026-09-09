//! Identity comes from a provider -- GitHub or Google. Several paths reach the
//! same place: a browser signs in through one provider's OAuth web flow and
//! carries a signed cookie afterwards, while a terminal signs in through this
//! deployment's own device flow and carries the token that produces as a
//! bearer. All of them end up as a handle, which the policies below either
//! allow or not, and an id, which everything else keys on.

use std::time::Duration;

use base64::Engine;
use hmac::{Hmac, Mac};
use sha2::{Digest, Sha256};

mod device;
mod github;
mod google;
pub mod pseudonym;

pub use device::*;
pub use github::*;
pub use google::*;

#[cfg(test)]
mod core_tests;

/// The two providers, spelled as the id prefix and the session cookie write
/// them. Nothing outside this module composes these strings by hand.
pub const PROVIDER_GITHUB: &str = "github";
pub const PROVIDER_GOOGLE: &str = "google";

pub const SESSION_COOKIE: &str = "librepaper_session";
pub const STATE_COOKIE: &str = "librepaper_state";
/// Names the browser itself, so an upload made without signing in still
/// belongs to whoever made it.
pub const VISITOR_COOKIE: &str = "librepaper_visitor";
pub const SESSION_MAX_AGE: Duration = Duration::from_secs(30 * 24 * 3600);

/// Added to every cookie name on an HTTPS request. A browser refuses to set a
/// __Host- cookie unless it also carries Secure, Path=/, and no Domain --
/// exactly how every cookie here is already set -- which is what keeps a
/// same-site subdomain from planting one.
pub const HOST_COOKIE_PREFIX: &str = "__Host-";

/// Who a caller is, once verified. `id` is what ownership, comment authorship
/// and grants key on, and it is qualified with the provider --
/// `github:583231`, `google:107691503500061507151` -- because GitHub ids and
/// Google `sub` values are both decimal strings and nothing but the prefix
/// keeps one namespace out of the other. `handle` is what the policies match:
/// a GitHub login, or a Google account's verified email. It needs no prefix,
/// since a GitHub login cannot contain `@` and a verified email always does,
/// so the character decides. `name` is what other readers see; it is never
/// matched on, and for a Google account it is deliberately not the email.
/// Every field is empty for an anonymous caller.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Identity {
    pub provider: String,
    pub id: String,
    pub handle: String,
    pub name: String,
    /// Random catalogue generation bound into signed credentials. Changing
    /// it revokes every cookie and device token for the account.
    pub session_generation: String,
}

impl Identity {
    pub fn anonymous() -> Identity {
        Identity::default()
    }

    /// The id is the one field nothing here works without, so it is the one
    /// that says whether anybody is signed in.
    pub fn is_signed_in(&self) -> bool {
        !self.id.is_empty()
    }

    /// A GitHub account, from its login and its numeric id. The login is both
    /// the handle and the displayed name: GitHub gives no other name to show,
    /// and the login is what a reader recognises.
    pub fn github(login: &str, id: &str) -> Identity {
        let login = login.trim().to_lowercase();
        Identity {
            provider: PROVIDER_GITHUB.to_string(),
            id: qualified(PROVIDER_GITHUB, id),
            name: login.clone(),
            handle: login,
            session_generation: String::new(),
        }
    }

    /// A Google account, from its `sub`, its verified email, and the profile
    /// name. Google returns no name for some accounts, and an unnamed comment
    /// is worse than one signed with the local part of the address, which is
    /// the name the person already writes to themselves under.
    pub fn google(sub: &str, email: &str, name: &str) -> Identity {
        let email = email.trim().to_lowercase();
        let name = name.trim();
        let shown = if name.is_empty() {
            email.split('@').next().unwrap_or_default().to_string()
        } else {
            name.to_string()
        };
        Identity {
            provider: PROVIDER_GOOGLE.to_string(),
            id: qualified(PROVIDER_GOOGLE, sub),
            handle: email,
            name: shown,
            session_generation: String::new(),
        }
    }
}

/// An id with its provider in front, unless it already carries one. Empty in,
/// empty out: no id at all is not an id belonging to anybody.
pub fn qualified(provider: &str, id: &str) -> String {
    let id = id.trim();
    if id.is_empty() || id.contains(':') {
        return id.to_string();
    }
    format!("{provider}:{id}")
}

/// What a bare id written by an earlier server means. Ownership, grants and
/// checkpoints were all written before providers existed, and everything
/// written then was GitHub, so a stored id with no prefix is read as one
/// rather than the index being rewritten.
pub fn stored_id(id: &str) -> String {
    qualified(PROVIDER_GITHUB, id)
}

/// The cookie name for this request: the __Host- prefix on HTTPS, the plain
/// name on HTTP (local `serve`), since __Host- is refused by browsers without
/// Secure. An HTTPS request must read only the prefixed name: the plain one is
/// exactly what a same-site document could plant in the reader's browser, so
/// falling back to it would defeat the point of the prefix.
pub fn cookie_name(https: bool, base: &str) -> String {
    if https {
        format!("{HOST_COOKIE_PREFIX}{base}")
    } else {
        base.to_string()
    }
}

/// Says who may do something. The default allows nobody, which is the right
/// default for publishing on a deployment that was never configured.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Policy {
    /// No sign-in at all; only meaningful for commenting.
    pub public: bool,
    /// Any account on any configured provider, once signed in. An operator who
    /// wants one provider only leaves the other unconfigured, which is a
    /// property of the deployment rather than of every policy on it.
    pub any: bool,
    /// Legacy `anygithub` retains its provider restriction.
    pub github_only: bool,
    /// The allowlist, lowercased and as written, when neither of the above is
    /// set. Each entry is a GitHub login, an email address, or `@domain`; the
    /// shape of the entry is what decides which, so the list stays one list.
    pub entries: Vec<String>,
}

impl Policy {
    /// Reads the value of --publishers or --commenters:
    ///
    ///     anyone                 no sign-in required at all
    ///     any                    any signed-in account, either provider
    ///     alice,bob              only these GitHub logins
    ///     alice@example.org      the Google account with that verified email
    ///     @example.org           any Google account on that domain
    ///
    /// The forms mix freely in one list.
    pub fn parse(value: &str) -> Policy {
        let trimmed = value.trim().to_lowercase();
        match trimmed.as_str() {
            "" => return Policy::default(),
            "anyone" | "public" => {
                return Policy {
                    public: true,
                    ..Policy::default()
                }
            }
            "anygithub" => {
                return Policy {
                    github_only: true,
                    ..Policy::default()
                };
            }
            "any" | "*" => {
                return Policy {
                    any: true,
                    ..Policy::default()
                }
            }
            _ => {}
        }
        let entries = trimmed
            .split(',')
            .map(str::trim)
            .filter(|entry| !entry.is_empty())
            .map(str::to_string)
            .collect();
        Policy {
            entries,
            ..Policy::default()
        }
    }

    /// Whether this policy admits a handle: a GitHub login, or a Google
    /// account's verified email. Matching is case-insensitive throughout,
    /// since neither a login nor an address is case-sensitive in practice.
    pub fn allows(&self, handle: &str) -> bool {
        if self.public {
            return true;
        }
        if handle.is_empty() {
            return false;
        }
        if self.any {
            return true;
        }
        if self.github_only {
            return !handle.contains('@');
        }
        self.entries.iter().any(|entry| entry_admits(entry, handle))
    }

    /// Whether anyone at all is allowed: an unconfigured policy is none of
    /// public, any, or a list.
    pub fn is_configured(&self) -> bool {
        self.public || self.any || self.github_only || !self.entries.is_empty()
    }

    /// The operator's startup summary, including configured allowlist entries.
    pub fn describe(&self) -> String {
        if self.public {
            "anyone".to_string()
        } else if self.any {
            "any signed-in account".to_string()
        } else if self.github_only {
            "any GitHub account".to_string()
        } else if self.entries.is_empty() {
            "nobody (unconfigured)".to_string()
        } else {
            self.entries.join(", ")
        }
    }

    /// Safe to show to ordinary callers, including accounts refused access.
    pub fn public_description(&self) -> String {
        if self.entries.is_empty() {
            self.describe()
        } else {
            format!(
                "an allowlist of {} {}",
                self.entries.len(),
                if self.entries.len() == 1 {
                    "entry"
                } else {
                    "entries"
                }
            )
        }
    }

    /// Whether this policy names anybody who could only sign in with GitHub,
    /// or only with Google. Startup uses these to warn about a list that names
    /// people no configured provider can ever produce.
    pub fn names_a_login(&self) -> bool {
        self.github_only || self.entries.iter().any(|entry| !entry.contains('@'))
    }

    pub fn names_an_email_or_domain(&self) -> bool {
        self.entries.iter().any(|entry| entry.contains('@'))
    }
}

/// One allowlist entry against one handle. A leading `@` and nothing before it
/// is a domain, and it matches the part after the `@` exactly, so
/// `@example.org` does not admit `@mail.example.org`; anything else is matched
/// whole, which covers both a login and a full address.
fn entry_admits(entry: &str, handle: &str) -> bool {
    if let Some(domain) = entry.strip_prefix('@') {
        return handle
            .split_once('@')
            .is_some_and(|(_, host)| host.eq_ignore_ascii_case(domain));
    }
    entry.eq_ignore_ascii_case(handle)
}

type HmacSha256 = Hmac<Sha256>;

fn base64url(bytes: &[u8]) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

/// Sign one explicit purpose. The separator prevents an ambiguous boundary
/// between a purpose and its payload. Purposes are fixed by the caller.
pub fn sign(key: &[u8], purpose: &str, payload: &str) -> String {
    let mut mac = HmacSha256::new_from_slice(key).expect("HMAC takes any key length");
    mac.update(purpose.as_bytes());
    mac.update(b"\0");
    mac.update(payload.as_bytes());
    base64url(&mac.finalize().into_bytes())
}

/// Constant-time verification within the same credential purpose.
pub fn verifies(key: &[u8], purpose: &str, payload: &str, signature: &str) -> bool {
    let Ok(given) = base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(signature) else {
        return false;
    };
    let mut mac = HmacSha256::new_from_slice(key).expect("HMAC takes any key length");
    mac.update(purpose.as_bytes());
    mac.update(b"\0");
    mac.update(payload.as_bytes());
    mac.verify_slice(&given).is_ok()
}

/// Browser and terminal credentials have separate MAC domains. Version one
/// deliberately rejects credentials from before purpose separation: accepting
/// those as a fallback would preserve their cross-use vulnerability.
pub fn sign_session(key: &[u8], id: &Identity, expiry_unix: i64) -> String {
    sign_identity(key, "session-v1", id, expiry_unix)
}

pub fn sign_device(key: &[u8], id: &Identity, expiry_unix: i64) -> String {
    format!(
        "{DEVICE_TOKEN_PREFIX}{}",
        sign_identity(key, "device-v1", id, expiry_unix)
    )
}

fn sign_identity(key: &[u8], purpose: &str, id: &Identity, expiry_unix: i64) -> String {
    let payload = base64url(
        format!(
            "{}|{}|{}|{}|{}|{}",
            id.provider, id.handle, id.id, id.session_generation, id.name, expiry_unix
        )
        .as_bytes(),
    );
    format!("v1.{payload}.{}", sign(key, purpose, &payload))
}

pub fn read_session(key: &[u8], cookie: &str) -> Identity {
    read_identity(key, "session-v1", cookie)
}

pub fn read_device(key: &[u8], token: &str) -> Identity {
    token
        .strip_prefix(DEVICE_TOKEN_PREFIX)
        .map(|token| read_identity(key, "device-v1", token))
        .unwrap_or_default()
}

fn read_identity(key: &[u8], purpose: &str, credential: &str) -> Identity {
    let Some(versioned) = credential.strip_prefix("v1.") else {
        return Identity::anonymous();
    };
    let Some((payload, signature)) = versioned.split_once('.') else {
        return Identity::anonymous();
    };
    if !verifies(key, purpose, payload, signature) {
        return Identity::anonymous();
    }
    let Ok(raw) = base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(payload) else {
        return Identity::anonymous();
    };
    let Ok(text) = String::from_utf8(raw) else {
        return Identity::anonymous();
    };
    // A profile name may contain bars, so only the last bar separates expiry.
    let Some((front, expiry)) = text.rsplit_once('|') else {
        return Identity::anonymous();
    };
    let Ok(expiry) = expiry.parse::<i64>() else {
        return Identity::anonymous();
    };
    if now_unix() >= expiry {
        return Identity::anonymous();
    }
    let fields: Vec<&str> = front.splitn(5, '|').collect();
    let [provider, handle, id, generation, name] = fields[..] else {
        return Identity::anonymous();
    };
    if !matches!(provider, PROVIDER_GITHUB | PROVIDER_GOOGLE)
        || handle.is_empty()
        || id
            .strip_prefix(&format!("{provider}:"))
            .is_none_or(str::is_empty)
    {
        return Identity::anonymous();
    }
    Identity {
        provider: provider.into(),
        handle: handle.into(),
        id: id.into(),
        session_generation: generation.into(),
        name: name.into(),
    }
}

pub fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// "<token>.<signature>" for a freshly minted visitor token, so a browser
/// cannot simply pick its own owner key.
pub fn sign_visitor(key: &[u8], token: &str) -> String {
    format!("v1.{token}.{}", sign(key, "visitor-v1", token))
}

/// The token a visitor cookie carries, or "" when the cookie is forged,
/// damaged, or in the old unsigned form a browser issued by an earlier server
/// might still hold. Treating that old form as absent means such a browser is
/// simply reissued a signed cookie, rather than kept on a value nothing here
/// can verify.
pub fn read_visitor(key: &[u8], cookie: &str) -> String {
    // Preserve ownership for anonymous browsers issued before v1. That format
    // only ever carried a 128-bit lowercase hex token. Enforcing its shape
    // excludes legacy sessions and frame capabilities from this migration.
    let Some(versioned) = cookie.strip_prefix("v1.") else {
        let Some((token, signature)) = cookie.split_once('.') else {
            return String::new();
        };
        if token.len() != 32
            || !token
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            return String::new();
        }
        let Ok(given) = base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(signature) else {
            return String::new();
        };
        let mut mac = HmacSha256::new_from_slice(key).expect("HMAC takes any key length");
        mac.update(token.as_bytes());
        return if mac.verify_slice(&given).is_ok() {
            token.to_string()
        } else {
            String::new()
        };
    };
    let Some((token, signature)) = versioned.split_once('.') else {
        return String::new();
    };
    if token.is_empty() || !verifies(key, "visitor-v1", token, signature) {
        return String::new();
    }
    token.to_string()
}

/// Load or durably create the local deployment signing key. Catalogue
/// deployments keep secrets outside the object namespace so a bucket listing
/// or object-store credential cannot disclose the cookie-signing material.
pub fn session_key_file(path: &std::path::Path, catalog_nonempty: bool) -> Result<Vec<u8>, String> {
    deployment_secret_key(path, catalog_nonempty, "session")
}

pub fn link_sealing_key_file(
    path: &std::path::Path,
    catalog_nonempty: bool,
) -> Result<Vec<u8>, String> {
    deployment_secret_key(path, catalog_nonempty, "link-sealing")
}

/// Versioned local link-key ring. The first key encrypts new envelopes; the
/// remainder are retained for interrupted rotations and backup restore. A
/// legacy single hex key is accepted as a one-entry ring.
pub fn link_sealing_keyring_file(
    path: &std::path::Path,
    catalog_nonempty: bool,
) -> Result<Vec<Vec<u8>>, String> {
    match std::fs::read(path) {
        Ok(raw) => {
            if let Some(key) = decode_session_key(&raw) {
                return Ok(vec![key]);
            }
            let value: serde_json::Value = serde_json::from_slice(&raw).map_err(|err| {
                format!("the link keyring at {} is invalid: {err}", path.display())
            })?;
            if value["version"].as_u64() != Some(1) {
                return Err(format!("unsupported link keyring at {}", path.display()));
            }
            let keys = value["keys"]
                .as_array()
                .ok_or_else(|| format!("the link keyring at {} has no keys", path.display()))?
                .iter()
                .map(|row| hex::decode(row["key"].as_str().unwrap_or("")))
                .collect::<Result<Vec<_>, _>>()
                .map_err(|_| {
                    format!(
                        "the link keyring at {} contains an invalid key",
                        path.display()
                    )
                })?;
            if keys.is_empty() || keys.iter().any(|key| key.len() != 32) {
                return Err(format!(
                    "the link keyring at {} contains an invalid key",
                    path.display()
                ));
            }
            Ok(keys)
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let primary = link_sealing_key_file(path, catalog_nonempty)?;
            Ok(vec![primary])
        }
        Err(error) => Err(format!("could not read {}: {error}", path.display())),
    }
}

pub fn write_link_sealing_keyring(path: &std::path::Path, keys: &[Vec<u8>]) -> Result<(), String> {
    if keys.is_empty() || keys.iter().any(|key| key.len() != 32) {
        return Err("link keyring needs 32-byte keys".into());
    }
    let rows: Vec<_> = keys
        .iter()
        .map(|key| {
            serde_json::json!({
                "id": hex::encode(Sha256::digest(key))[..16],
                "key": hex::encode(key),
            })
        })
        .collect();
    let body = serde_json::to_vec(&serde_json::json!({"version": 1, "keys": rows}))
        .map_err(|error| error.to_string())?;
    write_secret_atomically(path, &body, true).map(|_| ())
}

/// Publish fully written, private bytes, with optional replacement. Creating a
/// key uses a hard link so a concurrent creator cannot overwrite the winner.
/// Keyring replacement remains serialized by the deployment writer lock.
fn write_secret_atomically(
    path: &std::path::Path,
    body: &[u8],
    replace: bool,
) -> Result<bool, String> {
    use std::io::Write;
    let parent = path
        .parent()
        .ok_or_else(|| format!("{} has no parent", path.display()))?;
    std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    let temporary = path.with_extension(format!("tmp-{}", hex::encode(random_bytes(8))));
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options
        .open(&temporary)
        .map_err(|error| error.to_string())?;
    let result = (|| {
        file.write_all(body)?;
        file.sync_all()?;
        drop(file);
        let installed = if replace {
            std::fs::rename(&temporary, path)?;
            true
        } else {
            let installed = match std::fs::hard_link(&temporary, path) {
                Ok(()) => true,
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => false,
                Err(error) => return Err(error),
            };
            std::fs::remove_file(&temporary)?;
            installed
        };
        std::fs::File::open(parent)?.sync_all()?;
        Ok::<bool, std::io::Error>(installed)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    result.map_err(|error| format!("could not durably write {}: {error}", path.display()))
}

fn deployment_secret_key(
    path: &std::path::Path,
    catalog_nonempty: bool,
    purpose: &str,
) -> Result<Vec<u8>, String> {
    match std::fs::read(path) {
        Ok(raw) => {
            return decode_session_key(&raw)
                .ok_or_else(|| format!("the {purpose} key at {} is not readable", path.display()));
        }
        Err(err) if err.kind() == std::io::ErrorKind::NotFound && !catalog_nonempty => {}
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            return Err(format!(
                "the nonempty catalogue is missing its {purpose} key at {}",
                path.display()
            ));
        }
        Err(err) => return Err(format!("could not read {}: {err}", path.display())),
    }

    let key = random_bytes(32);
    if write_secret_atomically(path, hex::encode(&key).as_bytes(), false)? {
        return Ok(key);
    }
    let raw = std::fs::read(path)
        .map_err(|error| format!("could not read {}: {error}", path.display()))?;
    decode_session_key(&raw)
        .ok_or_else(|| format!("the {purpose} key at {} is not readable", path.display()))
}

/// Parses a hex-encoded 32-byte deployment key, or `None` for
/// anything else -- truncated, non-hex, the wrong length. A malformed value
/// is never "as good as absent": that equivalence is what let a transient
/// read failure look identical to an empty deployment.
fn decode_session_key(raw: &[u8]) -> Option<Vec<u8>> {
    let key = hex::decode(String::from_utf8_lossy(raw).trim()).ok()?;
    if key.len() == 32 {
        Some(key)
    } else {
        None
    }
}

pub fn random_bytes(n: usize) -> Vec<u8> {
    use rand::RngCore;
    let mut raw = vec![0u8; n];
    rand::rng().fill_bytes(&mut raw);
    raw
}

pub fn random_token() -> String {
    hex::encode(random_bytes(16))
}

/// A user code as the table keys it. People type them in whatever case their
/// keyboard is in, and a hyphen is the shape a printed code invites.
pub fn normalized(user: &str) -> String {
    user.trim()
        .to_uppercase()
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .collect()
}
