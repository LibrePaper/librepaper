//! Typed identifiers that arrive as text: project slugs, link keys, the
//! digests links are known by, and agent tokens. Each has one parser here.

use std::fmt;
use std::str::FromStr;

use base64::Engine;

use crate::auth::AGENT_TOKEN_PREFIX;
use crate::digest::Digest;
use crate::secret::Secret;
use crate::util::{token, Encoding};

/// The text was not the identifier it was parsed as.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum IdError {
    #[error("not a project slug")]
    Slug,
    #[error("not a link key")]
    LinkKey,
    #[error("not a link digest")]
    LinkDigest,
    #[error("not an agent token")]
    AgentToken,
}

const MAX_SLUG: usize = 200;

/// A project slug: 1 to 200 ASCII letters, digits, hyphens and underscores.
/// This is the syntax every caller agrees on. A deployment's own slug pattern
/// is a further check layered on top of it.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Slug(String);

impl Slug {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl FromStr for Slug {
    type Err = IdError;

    fn from_str(text: &str) -> Result<Slug, IdError> {
        let valid = !text.is_empty()
            && text.len() <= MAX_SLUG
            && text
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
        if valid {
            Ok(Slug(text.to_string()))
        } else {
            Err(IdError::Slug)
        }
    }
}

impl fmt::Display for Slug {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// The SHA-256 of a link key, which is all a document keeps of the key. Text
/// form is lowercase hex.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct LinkDigest(Digest);

impl LinkDigest {
    pub fn to_hex(&self) -> String {
        self.0.to_hex()
    }

    pub fn as_bytes(&self) -> &[u8; 32] {
        self.0.as_bytes()
    }
}

impl FromStr for LinkDigest {
    type Err = IdError;

    fn from_str(text: &str) -> Result<LinkDigest, IdError> {
        text.parse::<Digest>()
            .map(LinkDigest)
            .map_err(|_| IdError::LinkDigest)
    }
}

impl fmt::Display for LinkDigest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_hex())
    }
}

/// The secret in a share link or an agent token.
#[derive(Debug, Clone)]
pub struct LinkKey(Secret<String>);

impl LinkKey {
    /// A new key: 256 bits, written as 64 hex characters.
    pub fn mint() -> LinkKey {
        LinkKey(Secret::new(token::<32>(Encoding::Hex)))
    }

    /// The digest the key is known by. The key was trimmed when it was
    /// parsed, so this hashes it as it stands.
    pub fn digest(&self) -> LinkDigest {
        LinkDigest(Digest::of(self.0.expose().as_bytes()))
    }

    /// The key as text, for the one place that must put it in a URL.
    pub fn expose(&self) -> &str {
        self.0.expose()
    }
}

impl FromStr for LinkKey {
    type Err = IdError;

    /// Keys arrive from URLs and headers, so stray whitespace is trimmed and
    /// a key with nothing left is refused.
    fn from_str(text: &str) -> Result<LinkKey, IdError> {
        let text = text.trim();
        if text.is_empty() {
            return Err(IdError::LinkKey);
        }
        Ok(LinkKey(Secret::new(text.to_string())))
    }
}

/// `lpt_<base64url origin>.<slug>.<key>`: the app origin a page on the
/// deployment sees, the project slug and the link key. Only the slug and key
/// authorize. The origin says where to connect.
#[derive(Clone, PartialEq, Eq)]
pub struct AgentToken {
    pub origin: String,
    pub slug: Slug,
    pub key: String,
}

impl AgentToken {
    /// The digest the token's row is known by.
    pub fn digest(&self) -> LinkDigest {
        LinkDigest(Digest::of(self.key.as_bytes()))
    }
}

impl FromStr for AgentToken {
    type Err = IdError;

    /// The origin is base64url without padding and holds no dot, so the first
    /// dot ends it. The key holds no dot, so the last dot starts it. The slug
    /// between them is checked as a `Slug`.
    fn from_str(text: &str) -> Result<AgentToken, IdError> {
        let invalid = IdError::AgentToken;
        let rest = text.strip_prefix(AGENT_TOKEN_PREFIX).ok_or(invalid)?;
        let (origin, rest) = rest.split_once('.').ok_or(invalid)?;
        let (slug, key) = rest.rsplit_once('.').ok_or(invalid)?;
        if origin.is_empty() || key.is_empty() || key.chars().any(char::is_whitespace) {
            return Err(invalid);
        }
        let slug = slug.parse::<Slug>().map_err(|_| invalid)?;
        let decoded = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(origin)
            .ok()
            .and_then(|bytes| String::from_utf8(bytes).ok())
            .ok_or(invalid)?;
        let url = url::Url::parse(&decoded).map_err(|_| invalid)?;
        let is_origin = matches!(url.scheme(), "http" | "https")
            && url.host_str().is_some()
            && url.origin().ascii_serialization() == decoded;
        if !is_origin {
            return Err(invalid);
        }
        Ok(AgentToken {
            origin: decoded,
            slug,
            key: key.to_string(),
        })
    }
}

impl fmt::Display for AgentToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let origin = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(&self.origin);
        write!(f, "{AGENT_TOKEN_PREFIX}{origin}.{}.{}", self.slug, self.key)
    }
}

impl fmt::Debug for AgentToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AgentToken")
            .field("origin", &self.origin)
            .field("slug", &self.slug)
            .field("key", &"[redacted]")
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const KEY: &str = "0123456789abcdef0123456789abcdef";

    fn token_of(origin: &str, slug: &str, key: &str) -> String {
        let origin = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(origin);
        format!("lpt_{origin}.{slug}.{key}")
    }

    #[test]
    fn a_slug_is_letters_digits_hyphens_and_underscores() {
        for good in ["paper", "paper-abc", "Paper_2", &"a".repeat(200)] {
            assert_eq!(good.parse::<Slug>().unwrap().as_str(), good);
            assert_eq!(good.parse::<Slug>().unwrap().to_string(), good);
        }
        for bad in ["", "a.b", "a b", "a/b", "caf\u{e9}", &"a".repeat(201)] {
            assert!(bad.parse::<Slug>().is_err(), "{bad:?}");
        }
    }

    #[test]
    fn a_link_key_is_trimmed_and_never_empty() {
        let key: LinkKey = "  abc\n".parse().unwrap();
        assert_eq!(key.expose(), "abc");
        assert!("".parse::<LinkKey>().is_err());
        assert!(" \t\n".parse::<LinkKey>().is_err());
    }

    #[test]
    fn a_link_key_digest_is_the_sha256_in_hex() {
        let key: LinkKey = "abc".parse().unwrap();
        let hex = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";
        assert_eq!(key.digest().to_hex(), hex);
        assert_eq!(key.digest().to_string(), hex);
        assert_eq!(hex.parse::<LinkDigest>().unwrap(), key.digest());
        assert_eq!(key.digest().as_bytes(), &hex::decode(hex).unwrap()[..]);
    }

    #[test]
    fn a_bad_digest_is_refused() {
        assert!("abcd".parse::<LinkDigest>().is_err());
        assert!("".parse::<LinkDigest>().is_err());
        assert!("Z".repeat(64).parse::<LinkDigest>().is_err());
    }

    #[test]
    fn a_minted_key_is_64_hex_characters_and_never_printed() {
        let key = LinkKey::mint();
        assert_eq!(key.expose().len(), 64);
        assert!(key.expose().bytes().all(|b| b.is_ascii_hexdigit()));
        assert_ne!(key.expose(), LinkKey::mint().expose());
        assert_eq!(format!("{key:?}"), "LinkKey([redacted])");
        assert_eq!(key.expose().parse::<LinkKey>().unwrap().digest(), key.digest());
    }

    #[test]
    fn an_agent_token_round_trips() {
        let token = AgentToken {
            origin: "https://paper.example".to_string(),
            slug: "paper-abc".parse().unwrap(),
            key: KEY.to_string(),
        };
        let text = token.to_string();
        assert!(text.starts_with("lpt_"));
        assert_eq!(text.parse::<AgentToken>().unwrap(), token);
    }

    #[test]
    fn a_known_token_parses_to_known_parts() {
        let token: AgentToken = "lpt_aHR0cDovL2xvY2FsaG9zdDo4MDgx.paper.abcd1234"
            .parse()
            .unwrap();
        assert_eq!(token.origin, "http://localhost:8081");
        assert_eq!(token.slug.as_str(), "paper");
        assert_eq!(token.key, "abcd1234");
        assert_eq!(token.digest(), "abcd1234".parse::<LinkKey>().unwrap().digest());
        assert_eq!(
            token.to_string(),
            "lpt_aHR0cDovL2xvY2FsaG9zdDo4MDgx.paper.abcd1234"
        );
    }

    #[test]
    fn a_bad_origin_is_refused() {
        for token in [
            format!("lpt_!!!.paper.{KEY}"),
            format!("lpt_a=.paper.{KEY}"),
            token_of("ftp://paper.example", "paper", KEY),
            token_of("paper.example", "paper", KEY),
            token_of("https://paper.example/x", "paper", KEY),
        ] {
            assert!(token.parse::<AgentToken>().is_err(), "{token}");
        }
    }

    #[test]
    fn missing_parts_are_refused() {
        let origin = token_of("https://paper.example", "x", "y");
        let origin = origin
            .strip_prefix("lpt_")
            .and_then(|rest| rest.split_once('.'))
            .map(|(origin, _)| origin.to_string())
            .unwrap();
        for token in [
            "lpt_".to_string(),
            format!("lpt_{origin}"),
            format!("lpt_{origin}.{KEY}"),
            format!("lpt_{origin}..{KEY}"),
            format!("lpt_{origin}.paper."),
            format!("lpt_.paper.{KEY}"),
            format!("{origin}.paper.{KEY}"),
            format!("lpt_paper.{KEY}"),
        ] {
            assert!(token.parse::<AgentToken>().is_err(), "{token}");
        }
    }

    #[test]
    fn a_bad_slug_is_refused() {
        for slug in ["a.b", "a b", "a/b"] {
            let token = token_of("https://paper.example", slug, KEY);
            assert!(token.parse::<AgentToken>().is_err(), "{token}");
        }
    }

    #[test]
    fn an_agent_token_debug_hides_the_key() {
        let token: AgentToken = token_of("https://paper.example", "paper", KEY).parse().unwrap();
        assert!(!format!("{token:?}").contains(KEY));
    }
}
