//! Identity comes from a provider -- GitHub, or any OpenID Connect provider
//! (Google, Microsoft Entra ID, or one the operator names). Several paths reach
//! the same place: a browser signs in through one provider's web flow and
//! carries a signed cookie afterwards, while a terminal signs in through this
//! deployment's own device flow and carries the token that produces as a
//! bearer. All of them end up as a handle, which the policies below either
//! allow or not, and an id, which everything else keys on.

use std::collections::BTreeMap;
use std::time::Duration;

use base64::Engine;
use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use tokio::sync::{Semaphore, SemaphorePermit};

pub use crate::util::{now_unix, random_bytes};

mod device;
mod github;
mod oidc;
mod pkce;
pub mod pseudonym;

pub use device::*;
pub use github::*;
pub use oidc::*;
pub use pkce::*;

#[cfg(test)]
mod core_tests;

/// The provider ids, spelled as the id prefix and the session cookie write
/// them. Nothing outside this module composes these strings by hand. A custom
/// OpenID Connect provider is `OIDC_ID_PREFIX` followed by the operator's slug.
pub const PROVIDER_GITHUB: &str = "github";
pub const PROVIDER_GOOGLE: &str = "google";
pub const PROVIDER_ENTRA: &str = "entra";
pub const PROVIDER_ORCID: &str = "orcid";
pub const OIDC_ID_PREFIX: &str = "oidc-";

pub const SESSION_COOKIE: &str = "librepaper_session";
pub const STATE_COOKIE: &str = "librepaper_state";
/// Names the browser of an anonymous commenter. The server sets it only where
/// the deployment lets anyone comment.
pub const VISITOR_COOKIE: &str = "librepaper_visitor";
pub const SESSION_MAX_AGE: Duration = Duration::from_secs(30 * 24 * 3600);

/// How long any one call to a sign-in provider may take.
pub const PROVIDER_HTTP_TIMEOUT: Duration = Duration::from_secs(15);

/// Whether text is a slug an operator may give a custom provider: a lowercase
/// letter, then up to 23 lowercase letters, digits or hyphens.
pub fn valid_oidc_slug(slug: &str) -> bool {
    let bytes = slug.as_bytes();
    bytes.len() <= 24
        && bytes.first().is_some_and(u8::is_ascii_lowercase)
        && bytes[1..]
            .iter()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || *byte == b'-')
}

/// Whether text is a provider id this code can issue: one of the four named
/// providers, or `oidc-` and a valid slug.
pub fn valid_provider_id(id: &str) -> bool {
    matches!(
        id,
        PROVIDER_GITHUB | PROVIDER_GOOGLE | PROVIDER_ENTRA | PROVIDER_ORCID
    ) || id.strip_prefix(OIDC_ID_PREFIX).is_some_and(valid_oidc_slug)
}

/// A short-lived credential delegated by a signed-in browser to its local
/// companion. It is useful only for one document and one already-presented
/// share link. `role` is a ceiling, in the bytes `Role::wire` writes: 1 = reader,
/// 2 = commenter, 3 = editor.
/// The document server must still resolve the live link and deployment policy
/// on every request, and must treat this credential as automation even when
/// the caller omits the automation header.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentGrant {
    pub identity: Identity,
    pub slug: String,
    /// Lowercase hex SHA-256 digest of the share-link key, never the key.
    pub link_hash: String,
    pub role: u8,
    pub expires_at: i64,
}

/// Delegated credentials are deliberately much shorter lived than browser
/// sessions or device tokens. The browser renews them while its session lives.
pub const AGENT_GRANT_MAX_AGE: Duration = Duration::from_secs(5 * 60);
pub const AGENT_GRANT_PREFIX: &str = "lpa_";

/// Owner-issued agent tokens: `lpt_<slug>.<key>`. Unlike a delegated grant a
/// token is not signed. The key is looked up by digest on the document's row.
pub const AGENT_TOKEN_PREFIX: &str = "lpt_";

#[derive(Serialize, Deserialize)]
struct AgentGrantPayload {
    provider: String,
    id: String,
    handle: String,
    name: String,
    picture: String,
    session_generation: String,
    slug: String,
    link_hash: String,
    role: u8,
    expires_at: i64,
}

/// Sign a document-bound grant for the local companion. Invalid or overly
/// broad claims cannot be minted through this API.
pub fn sign_agent_grant(key: &[u8], grant: &AgentGrant) -> Option<String> {
    if !valid_agent_grant(grant, now_unix()) {
        return None;
    }
    let payload = AgentGrantPayload {
        provider: grant.identity.provider.clone(),
        id: grant.identity.id.clone(),
        handle: grant.identity.handle.clone(),
        name: grant.identity.name.clone(),
        picture: acceptable_picture(&grant.identity.picture),
        session_generation: grant.identity.session_generation.clone(),
        slug: grant.slug.clone(),
        link_hash: grant.link_hash.clone(),
        role: grant.role,
        expires_at: grant.expires_at,
    };
    let encoded = base64url(&serde_json::to_vec(&payload).ok()?);
    let signature = sign(key, "agent-grant-v1", &encoded);
    Some(format!("{AGENT_GRANT_PREFIX}v1.{encoded}.{signature}"))
}

/// Verify a delegated credential. Its signature, account generation, slug,
/// link digest and role are all returned as claims for the server's request
/// authorization layer to check against current catalogue state.
pub fn read_agent_grant(key: &[u8], token: &str) -> Option<AgentGrant> {
    let token = token
        .strip_prefix(AGENT_GRANT_PREFIX)?
        .strip_prefix("v1.")?;
    let (encoded, signature) = token.split_once('.')?;
    if !verifies(key, "agent-grant-v1", encoded, signature) {
        return None;
    }
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(encoded)
        .ok()?;
    let payload: AgentGrantPayload = serde_json::from_slice(&bytes).ok()?;
    let identity = Identity {
        provider: payload.provider,
        id: payload.id,
        handle: payload.handle,
        name: payload.name,
        picture: acceptable_picture(&payload.picture),
        session_generation: payload.session_generation,
    };
    let grant = AgentGrant {
        identity,
        slug: payload.slug,
        link_hash: payload.link_hash,
        role: payload.role,
        expires_at: payload.expires_at,
    };
    valid_agent_grant(&grant, now_unix()).then_some(grant)
}

fn valid_agent_grant(grant: &AgentGrant, now: i64) -> bool {
    let identity = &grant.identity;
    identity.is_signed_in()
        && !identity.session_generation.is_empty()
        && valid_provider_id(&identity.provider)
        && !identity.handle.is_empty()
        && crate::role::Role::try_from(grant.role).is_ok()
        && !grant.slug.is_empty()
        && grant.slug.len() <= 128
        && grant
            .slug
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"_-".contains(&byte))
        && grant.link_hash.len() == 64
        && grant.link_hash.bytes().all(|byte| byte.is_ascii_hexdigit())
        && grant
            .link_hash
            .bytes()
            .all(|byte| !byte.is_ascii_uppercase())
        && grant.expires_at > now
        && grant.expires_at <= now + AGENT_GRANT_MAX_AGE.as_secs() as i64
}

/// Added to every cookie name on an HTTPS request. A browser refuses to set a
/// __Host- cookie unless it also carries Secure, Path=/, and no Domain --
/// exactly how every cookie here is already set -- which is what keeps a
/// same-site subdomain from planting one.
pub const HOST_COOKIE_PREFIX: &str = "__Host-";

/// All provider exchanges and profile lookups share one small admission pool.
/// A provider outage must not turn a burst of sign-in callbacks or owner
/// mutations into an unbounded set of waiting HTTP requests.
#[cfg(not(test))]
const PROVIDER_REQUEST_CONCURRENCY: usize = 16;
// `cargo test` runs this crate's tests as threads of one process, so the pool
// is shared by every provider test at once and a machine with more than 16
// cores sees Busy. No test here exercises the limit; the server crate's test
// that drains it builds this crate without cfg(test) and sees 16.
#[cfg(test)]
const PROVIDER_REQUEST_CONCURRENCY: usize = 1024;
static PROVIDER_REQUESTS: Semaphore = Semaphore::const_new(PROVIDER_REQUEST_CONCURRENCY);

pub fn try_provider_request() -> Option<SemaphorePermit<'static>> {
    PROVIDER_REQUESTS.try_acquire().ok()
}

/// Who a caller is, once verified. `id` is what ownership, comment authorship
/// and grants key on, and it is qualified with the provider --
/// `github:583231`, `google:107691503500061507151` -- because GitHub ids and
/// Google `sub` values are both decimal strings and nothing but the prefix
/// keeps one namespace out of the other. `handle` is what the policies match:
/// a GitHub login, or an email address a configured provider vouches for. It
/// needs no prefix, since a GitHub login cannot contain `@` and such an email
/// always does, so the character decides. `name` is what other readers see;
/// it is never matched on, and for an email provider it is deliberately not
/// the email. Every field is empty for an anonymous caller.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Identity {
    pub provider: String,
    pub id: String,
    pub handle: String,
    pub name: String,
    /// The provider's profile picture, as an `https://` URL, or empty. Stored
    /// only for OpenID Connect providers, whose picture URLs are opaque; a
    /// GitHub avatar is a function of the numeric id, so `picture_url`
    /// derives it and the credential need not carry it.
    pub picture: String,
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
        Identity::from_provider(PROVIDER_GITHUB, id, &login, &login, "")
    }

    /// An account from any provider: its subject, the handle the policies
    /// match (a login or a verified email), the name to show and a picture.
    /// The id is the provider and the subject joined with a colon. Nothing is
    /// done to a subject that already has a colon in it, because a subject
    /// may, and the provider in front is what keeps it out of any other
    /// namespace. A provider may send no name, or an address as the name, and a
    /// comment signed with an address is worse than one signed with its local
    /// part, so an address-shaped name is cut to its own local part and a
    /// missing name falls back to the handle's. No subject means no id, so
    /// nobody is signed in. The picture
    /// is kept only if it is an address a page could safely show and a
    /// credential could safely carry.
    pub fn from_provider(
        provider: &str,
        subject: &str,
        handle: &str,
        name: &str,
        picture: &str,
    ) -> Identity {
        let handle = handle.trim().to_lowercase();
        let name: String = name.chars().filter(|c| !c.is_control()).collect();
        let name: String = name.trim().chars().take(MAX_NAME_CHARS).collect();
        let name = name.trim_end();
        let name = match name.split_once('@') {
            Some((local, _)) => local.trim_end().to_string(),
            None => name.to_string(),
        };
        let name = if name.is_empty() {
            handle.split('@').next().unwrap_or_default().to_string()
        } else {
            name
        };
        // The subject is kept exactly as the provider sent it. Trimming it
        // would fold two subjects the provider tells apart into one account.
        Identity {
            provider: provider.to_string(),
            id: if subject.trim().is_empty() {
                String::new()
            } else {
                format!("{provider}:{subject}")
            },
            handle,
            name,
            picture: acceptable_picture(picture),
            session_generation: String::new(),
        }
    }

    /// Where the account's picture can be fetched from, or empty when the
    /// provider offers none. A GitHub avatar lives at a URL made of the
    /// numeric id alone -- it is exactly what `avatar_url` on `/user` answers
    /// with -- so it costs nothing to keep, and every session that exists
    /// today already has one.
    pub fn picture_url(&self) -> String {
        if !self.picture.is_empty() {
            return self.picture.clone();
        }
        if self.provider == PROVIDER_GITHUB {
            if let Some(number) = self.id.strip_prefix("github:") {
                if !number.is_empty() && number.bytes().all(|byte| byte.is_ascii_digit()) {
                    return format!("https://avatars.githubusercontent.com/u/{number}?s=96&v=4");
                }
            }
        }
        String::new()
    }
}

/// The longest display name an identity carries, in characters.
const MAX_NAME_CHARS: usize = 100;

/// A picture URL a provider answered with, or empty if it is not one a page
/// should be handed: it has to be https, short, made of URL characters, and
/// free of the bar that separates credential fields.
fn acceptable_picture(picture: &str) -> String {
    let picture = picture.trim();
    let plain = picture.starts_with("https://")
        && picture.len() <= 512
        && picture
            .bytes()
            .all(|byte| byte.is_ascii_graphic() && !matches!(byte, b'|' | b'"' | b'<' | b'>'));
    if plain {
        picture.to_string()
    } else {
        String::new()
    }
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
/// default for publishing on a deployment that was never configured. A list
/// belongs to one provider, so an entry can only admit accounts that provider
/// vouches for: a GitHub login never admits an ORCID iD with the same text.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Policy {
    /// No sign-in at all; only meaningful for commenting.
    pub public: bool,
    /// Provider id ("github", "google", "entra", "orcid", "oidc-<slug>") to its
    /// entries, already normalized by `normalize_entry`. "*" admits any account
    /// of that provider.
    pub providers: BTreeMap<String, Vec<String>>,
}

impl Policy {
    /// Nobody signs in; anyone may act.
    pub fn anonymous() -> Policy {
        Policy {
            public: true,
            ..Policy::default()
        }
    }

    /// A policy with one provider's list.
    pub fn allowing(provider: &str, entries: &[&str]) -> Policy {
        Policy::default().with(provider, entries)
    }

    /// This policy with one provider's list added, replacing any list that
    /// provider already had.
    pub fn with(mut self, provider: &str, entries: &[&str]) -> Policy {
        self.providers.insert(
            provider.to_string(),
            entries.iter().map(|entry| entry.to_string()).collect(),
        );
        self
    }

    /// Whether this policy admits an identity. The provider picks the list and
    /// the handle is matched against it case-insensitively, since neither a
    /// login nor an address is case-sensitive in practice.
    pub fn allows(&self, who: &Identity) -> bool {
        if self.public {
            return true;
        }
        if !who.is_signed_in() || who.handle.is_empty() {
            return false;
        }
        self.providers.get(&who.provider).is_some_and(|entries| {
            entries
                .iter()
                .any(|entry| entry == "*" || entry_admits(entry, &who.handle))
        })
    }

    /// Whether anyone at all is allowed: an unconfigured policy is neither
    /// public nor a non-empty list.
    pub fn is_configured(&self) -> bool {
        self.public || self.providers.values().any(|entries| !entries.is_empty())
    }

    /// The operator's startup summary, including configured allowlist entries.
    pub fn describe(&self) -> String {
        if self.public {
            return "anyone".to_string();
        }
        let lists: Vec<String> = self
            .providers
            .iter()
            .filter(|(_, entries)| !entries.is_empty())
            .map(|(provider, entries)| format!("{provider}: {}", entries.join(", ")))
            .collect();
        if lists.is_empty() {
            "nobody".to_string()
        } else {
            lists.join("; ")
        }
    }

    /// Safe to show to ordinary callers, including accounts refused access.
    pub fn public_description(&self) -> String {
        if self.public {
            return "anyone".to_string();
        }
        let lists: Vec<(&String, &Vec<String>)> = self
            .providers
            .iter()
            .filter(|(_, entries)| !entries.is_empty())
            .collect();
        match lists.as_slice() {
            [] => "nobody".to_string(),
            [(provider, entries)] if entries.as_slice() == ["*"] => {
                format!("any account of {provider}")
            }
            _ => {
                let count: usize = lists.iter().map(|(_, entries)| entries.len()).sum();
                format!(
                    "an allowlist of {count} {}",
                    if count == 1 { "entry" } else { "entries" }
                )
            }
        }
    }
}

/// Checks one allowlist entry for a provider and returns it in the form
/// `Policy` matches. A GitHub entry is a login, an ORCID entry is an iD, and
/// every other provider vouches for email addresses, so its entry is an address
/// or an `@domain`. `*` admits any account of the provider.
pub fn normalize_entry(provider: &str, entry: &str) -> Result<String, String> {
    let entry = entry.trim();
    if entry == "*" {
        return Ok("*".to_string());
    }
    match provider {
        PROVIDER_GITHUB => {
            if entry.starts_with('@') {
                return Err("write the login without @".to_string());
            }
            let login = entry.to_lowercase();
            let valid = (1..=39).contains(&login.len())
                && login
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
                && !login.starts_with('-')
                && !login.ends_with('-');
            if valid {
                Ok(login)
            } else {
                Err("expected a GitHub login or *".to_string())
            }
        }
        PROVIDER_ORCID => {
            if is_orcid_id(entry) {
                Ok(entry.to_uppercase())
            } else {
                Err("expected an ORCID iD such as 0000-0002-1825-0097, or *".to_string())
            }
        }
        _ => {
            let address = entry.to_lowercase();
            let error = || "expected an email address, @domain or *".to_string();
            if address.chars().any(char::is_whitespace) || address.matches('@').count() != 1 {
                return Err(error());
            }
            let (local, domain) = address.split_once('@').unwrap_or_default();
            let valid = if local.is_empty() {
                domain.contains('.')
            } else {
                !domain.is_empty()
            };
            if valid {
                Ok(address)
            } else {
                Err(error())
            }
        }
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
///
/// Version two carries the complete current identity envelope. The version is
/// part of the signed purpose, so a payload relabelled with another version
/// fails its signature instead of being parsed with shifted fields.
pub fn sign_session(key: &[u8], id: &Identity, expiry_unix: i64) -> String {
    sign_identity(key, "session", id, expiry_unix)
}

pub fn sign_device(key: &[u8], id: &Identity, expiry_unix: i64) -> String {
    format!(
        "{DEVICE_TOKEN_PREFIX}{}",
        sign_identity(key, "device", id, expiry_unix)
    )
}

fn sign_identity(key: &[u8], purpose: &str, id: &Identity, expiry_unix: i64) -> String {
    // The picture goes before the name: only the name may contain bars.
    let payload = base64url(
        format!(
            "{}|{}|{}|{}|{}|{}|{}",
            id.provider, id.handle, id.id, id.session_generation, id.picture, id.name, expiry_unix
        )
        .as_bytes(),
    );
    format!(
        "v2.{payload}.{}",
        sign(key, &format!("{purpose}-v2"), &payload)
    )
}

pub fn read_session(key: &[u8], cookie: &str) -> Identity {
    read_identity(key, "session", cookie)
}

pub fn read_device(key: &[u8], token: &str) -> Identity {
    token
        .strip_prefix(DEVICE_TOKEN_PREFIX)
        .map(|token| read_identity(key, "device", token))
        .unwrap_or_default()
}

fn read_identity(key: &[u8], purpose: &str, credential: &str) -> Identity {
    read_credential(key, purpose, credential)
        .map(|(identity, _)| identity)
        .unwrap_or_default()
}

/// When a session cookie stops being valid, for one that is valid now. A
/// credential approved from another must never outlive it, so the approver's
/// deadline is read here and caps whatever it approves.
pub fn session_deadline(key: &[u8], cookie: &str) -> Option<i64> {
    read_credential(key, "session", cookie).map(|(_, deadline)| deadline)
}

/// The same for a device token.
pub fn device_deadline(key: &[u8], token: &str) -> Option<i64> {
    let token = token.strip_prefix(DEVICE_TOKEN_PREFIX)?;
    read_credential(key, "device", token).map(|(_, deadline)| deadline)
}

fn read_credential(key: &[u8], purpose: &str, credential: &str) -> Option<(Identity, i64)> {
    let versioned = credential.strip_prefix("v2.")?;
    let (payload, signature) = versioned.split_once('.')?;
    if !verifies(key, &format!("{purpose}-v2"), payload, signature) {
        return None;
    }
    let raw = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(payload)
        .ok()?;
    let text = String::from_utf8(raw).ok()?;
    // A profile name may contain bars, so only the last bar separates expiry.
    let (front, expiry) = text.rsplit_once('|')?;
    let expiry = expiry.parse::<i64>().ok()?;
    if now_unix() >= expiry {
        return None;
    }
    let fields: Vec<&str> = front.splitn(6, '|').collect();
    let [provider, handle, id, generation, picture, name] = fields[..] else {
        return None;
    };
    let provider_identity = id
        .strip_prefix(&format!("{provider}:"))
        .is_some_and(|subject| !subject.is_empty());
    let catalog_identity = uuid::Uuid::parse_str(id).is_ok();
    if !valid_provider_id(provider)
        || handle.is_empty()
        || (!provider_identity && !catalog_identity)
    {
        return None;
    }
    let identity = Identity {
        provider: provider.into(),
        handle: handle.into(),
        id: id.into(),
        session_generation: generation.into(),
        picture: acceptable_picture(picture),
        name: name.into(),
    };
    Some((identity, expiry))
}

/// "<token>.<signature>" for a freshly minted visitor token, so a browser
/// cannot simply pick its own owner key.
pub fn sign_visitor(key: &[u8], token: &str) -> String {
    format!("v1.{token}.{}", sign(key, "visitor-v1", token))
}

/// The token a visitor cookie carries, or "" when the cookie is forged or
/// damaged.
pub fn read_visitor(key: &[u8], cookie: &str) -> String {
    let Some(versioned) = cookie.strip_prefix("v1.") else {
        return String::new();
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

/// StoreBundle fully written, private bytes, with optional replacement. Creating a
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
                "the existing deployment is missing its {purpose} signing key at {}; restore the matching secrets/{} from its backup before starting the app. Refusing to generate a replacement because the lost key could invalidate existing credentials and prevent recovery of saved share-link secrets; share URLs already held by users remain usable",
                path.display(),
                path.file_name().and_then(|name| name.to_str()).unwrap_or("session.key")
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

/// A user code as the table keys it. People type them in whatever case their
/// keyboard is in, and a hyphen is the shape a printed code invites.
pub fn normalized(user: &str) -> String {
    user.trim()
        .to_uppercase()
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .collect()
}
