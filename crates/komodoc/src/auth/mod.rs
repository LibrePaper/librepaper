//! Identity comes from a provider -- GitHub or Google. Several paths reach the
//! same place: a browser signs in through one provider's OAuth web flow and
//! carries a signed cookie afterwards, while a terminal signs in through this
//! deployment's own device flow and carries the token that produces as a
//! bearer. All of them end up as a handle, which the policies below either
//! allow or not, and an id, which everything else keys on.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use base64::Engine;
use hmac::{Hmac, Mac};
use serde::Deserialize;
use sha2::{Digest, Sha256};

use crate::http::{client, USER_AGENT};
use crate::storage::blob::{BlobError, BlobStore, SESSION_KEY_KEY};

mod device;
mod github;
mod google;
pub mod pseudonym;

pub use device::*;
pub use github::*;
pub use google::*;

/// The two providers, spelled as the id prefix and the session cookie write
/// them. Nothing outside this module composes these strings by hand.
pub const PROVIDER_GITHUB: &str = "github";
pub const PROVIDER_GOOGLE: &str = "google";

pub const SESSION_COOKIE: &str = "komodoc_session";
pub const STATE_COOKIE: &str = "komodoc_state";
/// Names the browser itself, so an upload made without signing in still
/// belongs to whoever made it.
pub const VISITOR_COOKIE: &str = "komodoc_visitor";
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
            "any" | "*" | "anygithub" => {
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
        self.entries.iter().any(|entry| entry_admits(entry, handle))
    }

    /// Whether anyone at all is allowed: an unconfigured policy is none of
    /// public, any, or a list.
    pub fn is_configured(&self) -> bool {
        self.public || self.any || !self.entries.is_empty()
    }

    /// What the page shows when someone is refused. The entries are shown as
    /// they were written, since an email and a domain already carry the `@`
    /// that tells them apart from a login.
    pub fn describe(&self) -> String {
        if self.public {
            "anyone".to_string()
        } else if self.any {
            "any signed-in account".to_string()
        } else if self.entries.is_empty() {
            "nobody (unconfigured)".to_string()
        } else {
            self.entries.join(", ")
        }
    }

    /// Whether this policy names anybody who could only sign in with GitHub,
    /// or only with Google. Startup uses these to warn about a list that names
    /// people no configured provider can ever produce.
    pub fn names_a_login(&self) -> bool {
        self.entries.iter().any(|entry| !entry.contains('@'))
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

pub fn sign(key: &[u8], payload: &str) -> String {
    let mut mac = HmacSha256::new_from_slice(key).expect("HMAC takes any key length");
    mac.update(payload.as_bytes());
    base64url(&mac.finalize().into_bytes())
}

/// Constant-time check of a signature against what the key says it should be.
pub fn verifies(key: &[u8], payload: &str, signature: &str) -> bool {
    let Ok(given) = base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(signature) else {
        return false;
    };
    let mut mac = HmacSha256::new_from_slice(key).expect("HMAC takes any key length");
    mac.update(payload.as_bytes());
    mac.verify_slice(&given).is_ok()
}

/// Returns "<payload>.<signature>", where the payload is the provider, the
/// handle, the qualified id, the displayed name, and an expiry. Nothing is
/// stored server-side: the signature is what makes it trustworthy. The name
/// rides along so that rendering the nav never needs a round trip to the
/// provider to find out what to label it with.
pub fn sign_session(key: &[u8], id: &Identity, expiry_unix: i64) -> String {
    let payload = base64url(
        format!(
            "{}|{}|{}|{}|{}|{}",
            id.provider, id.handle, id.id, id.session_generation, id.name, expiry_unix
        )
        .as_bytes(),
    );
    let signature = sign(key, &payload);
    format!("{payload}.{signature}")
}

/// The identity a cookie carries, or the anonymous identity if it is forged,
/// damaged, expired, or in the old two-field shape.
///
/// Three shapes reach here. The five-field one is what this server writes. The
/// three-field one -- `login|id|expiry` -- is a session an earlier server
/// wrote, which was necessarily GitHub, and is read as one until it expires:
/// nothing is missing from it, only implied. The two-field one, from before
/// the id existed at all, is still refused rather than half-trusted, because
/// it carries no id to check ownership or comment authorship against.
pub fn read_session(key: &[u8], cookie: &str) -> Identity {
    let Some((payload, signature)) = cookie.split_once('.') else {
        return Identity::anonymous();
    };
    if !verifies(key, payload, signature) {
        return Identity::anonymous();
    }
    let Ok(raw) = base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(payload) else {
        return Identity::anonymous();
    };
    let Ok(text) = String::from_utf8(raw) else {
        return Identity::anonymous();
    };
    // A profile name may itself contain a bar, so the expiry is taken off the
    // end and the rest split from the front; the name is whatever is left.
    let Some((front, expiry)) = text.rsplit_once('|') else {
        return Identity::anonymous();
    };
    let Ok(expiry) = expiry.parse::<i64>() else {
        return Identity::anonymous();
    };
    if now_unix() > expiry {
        return Identity::anonymous();
    }
    let fields: Vec<&str> = front.splitn(5, '|').collect();
    let who = match fields[..] {
        [provider, handle, id, generation, name] => Identity {
            provider: provider.to_string(),
            id: id.to_string(),
            handle: handle.to_string(),
            name: name.to_string(),
            session_generation: generation.to_string(),
        },
        // Legacy signed sessions remain parseable for JSON-backed test and
        // migration tooling. Catalogue-backed request validation rejects the
        // missing generation.
        [provider, handle, id, name] => Identity {
            provider: provider.to_string(),
            id: id.to_string(),
            handle: handle.to_string(),
            name: name.to_string(),
            session_generation: String::new(),
        },
        [login, id] => Identity::github(login, id),
        _ => return Identity::anonymous(),
    };
    if !who.is_signed_in() {
        return Identity::anonymous();
    }
    who
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
    format!("{token}.{}", sign(key, token))
}

/// The token a visitor cookie carries, or "" when the cookie is forged,
/// damaged, or in the old unsigned form a browser issued by an earlier server
/// might still hold. Treating that old form as absent means such a browser is
/// simply reissued a signed cookie, rather than kept on a value nothing here
/// can verify.
pub fn read_visitor(key: &[u8], cookie: &str) -> String {
    let Some((token, signature)) = cookie.split_once('.') else {
        return String::new();
    };
    if token.is_empty() || !verifies(key, token, signature) {
        return String::new();
    }
    token.to_string()
}

/// What session and visitor cookies are signed with. It lives with the
/// documents rather than beside them: a server whose storage is a bucket keeps
/// nothing locally, and a key that did not survive a restart would sign every
/// reader out on every deploy. It is a secret in the operator's own storage,
/// which is the same trust the documents are already under.
#[cfg_attr(not(test), allow(dead_code))]
pub async fn session_key(blobs: &dyn BlobStore) -> Result<Vec<u8>, String> {
    match blobs.get(SESSION_KEY_KEY).await {
        // A key is already there: use it, or refuse to run over it. Either
        // way this is not the "nothing has ever been written" case that
        // justifies minting a replacement -- a byte flipped in storage, or a
        // read that came back short, must not look like an empty deployment.
        Ok(raw) => {
            return decode_session_key(&raw).ok_or_else(|| {
                format!(
                    "the session key stored at {} is not readable; refusing to replace it",
                    blobs.describe()
                )
            });
        }
        // The one case that means "nobody has ever put a key here".
        Err(BlobError::NotFound) => {}
        // Anything else -- a timeout, a permissions error, a bucket that is
        // momentarily unreachable -- must not be treated as "there is no
        // key yet". Doing so is exactly how a transient GET failure used to
        // rotate the deployment's signing key and sign everyone out; failing
        // startup here is the safe answer instead.
        Err(err) => {
            return Err(format!(
                "could not read the session key from {}: {err}",
                blobs.describe()
            ));
        }
    }
    let key = random_bytes(32);
    // Created with a compare-and-set against absence (the empty `expect`),
    // not an unconditional `put`, so two servers starting at once cannot each
    // write their own key and disagree forever: the loser re-reads and uses
    // whichever key actually won.
    match blobs
        .swap(SESSION_KEY_KEY, hex::encode(&key).into_bytes(), "")
        .await
    {
        Ok(_) => Ok(key),
        Err(BlobError::Conflict) => {
            let raw = blobs.get(SESSION_KEY_KEY).await.map_err(|err| {
                format!(
                    "could not read the session key from {} after losing its creation: {err}",
                    blobs.describe()
                )
            })?;
            decode_session_key(&raw).ok_or_else(|| {
                format!(
                    "the session key stored at {} is not readable; refusing to replace it",
                    blobs.describe()
                )
            })
        }
        Err(err) => Err(format!(
            "could not write the session key to {}: {err}",
            blobs.describe()
        )),
    }
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
    use std::io::Write;
    if keys.is_empty() || keys.iter().any(|key| key.len() != 32) {
        return Err("link keyring needs 32-byte keys".into());
    }
    let body=serde_json::to_vec(&serde_json::json!({"version":1,"keys":keys.iter().map(|key|serde_json::json!({"id":hex::encode(sha2::Sha256::digest(key))[..16].to_string(),"key":hex::encode(key)})).collect::<Vec<_>>() })).map_err(|e|e.to_string())?;
    let parent = path
        .parent()
        .ok_or_else(|| format!("{} has no parent", path.display()))?;
    let temporary = path.with_extension(format!("tmp-{}", hex::encode(random_bytes(8))));
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(&temporary).map_err(|e| e.to_string())?;
    let result = (|| {
        file.write_all(&body)?;
        file.sync_all()?;
        std::fs::rename(&temporary, path)?;
        std::fs::File::open(parent)?.sync_all()?;
        Ok::<_, std::io::Error>(())
    })();
    if let Err(error) = result {
        let _ = std::fs::remove_file(&temporary);
        return Err(format!(
            "could not durably update {}: {error}",
            path.display()
        ));
    }
    Ok(())
}

fn deployment_secret_key(
    path: &std::path::Path,
    catalog_nonempty: bool,
    purpose: &str,
) -> Result<Vec<u8>, String> {
    use std::io::Write;

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

    let parent = path
        .parent()
        .ok_or_else(|| format!("{} has no parent directory", path.display()))?;
    std::fs::create_dir_all(parent)
        .map_err(|err| format!("could not create {}: {err}", parent.display()))?;
    let key = random_bytes(32);
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
        .map_err(|err| format!("could not create {}: {err}", temporary.display()))?;
    let result = (|| {
        file.write_all(hex::encode(&key).as_bytes())?;
        file.sync_all()?;
        std::fs::rename(&temporary, path)?;
        std::fs::File::open(parent)?.sync_all()?;
        Ok::<(), std::io::Error>(())
    })();
    if let Err(err) = result {
        let _ = std::fs::remove_file(&temporary);
        return Err(format!(
            "could not durably create {}: {err}",
            path.display()
        ));
    }
    Ok(key)
}

/// Parses the hex-encoded 32-byte key `session_key` stores, or `None` for
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
