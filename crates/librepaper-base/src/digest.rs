//! One SHA-256 digest type. Text forms are lowercase hex, 64 characters.

use std::fmt;
use std::io::Read;
use std::str::FromStr;

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use sha2::{Digest as _, Sha256};

#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Digest([u8; 32]);

/// The text or bytes were not a SHA-256 digest.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("not a sha-256 digest")]
pub struct DigestError;

impl Digest {
    pub fn of(bytes: &[u8]) -> Self {
        Self(Sha256::digest(bytes).into())
    }

    pub fn of_reader(mut r: impl Read) -> std::io::Result<Self> {
        let mut hasher = Sha256::new();
        let mut buf = [0u8; 64 * 1024];
        loop {
            let n = match r.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => n,
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(e) => return Err(e),
            };
            hasher.update(&buf[..n]);
        }
        Ok(Self(hasher.finalize().into()))
    }

    pub fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    pub fn to_hex(&self) -> String {
        hex::encode(self.0)
    }

    /// The first `hex_chars` hex characters, for display ids only.
    pub fn prefix(&self, hex_chars: usize) -> String {
        let mut hex = self.to_hex();
        hex.truncate(hex_chars);
        hex
    }
}

impl TryFrom<&[u8]> for Digest {
    type Error = DigestError;

    fn try_from(bytes: &[u8]) -> Result<Self, DigestError> {
        <[u8; 32]>::try_from(bytes)
            .map(Self)
            .map_err(|_| DigestError)
    }
}

impl FromStr for Digest {
    type Err = DigestError;

    fn from_str(text: &str) -> Result<Self, DigestError> {
        if text.len() != 64 || !text.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')) {
            return Err(DigestError);
        }
        let mut out = [0u8; 32];
        hex::decode_to_slice(text, &mut out).map_err(|_| DigestError)?;
        Ok(Self(out))
    }
}

impl fmt::Display for Digest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_hex())
    }
}

impl fmt::Debug for Digest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_hex())
    }
}

impl Serialize for Digest {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.to_hex())
    }
}

impl<'de> Deserialize<'de> for Digest {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        text.parse().map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ABC: &str = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";

    #[test]
    fn known_vector() {
        assert_eq!(Digest::of(b"abc").to_hex(), ABC);
        assert_eq!(Digest::of_reader(&b"abc"[..]).unwrap(), Digest::of(b"abc"));
        assert_eq!(Digest::of(b"abc").prefix(8), "ba7816bf");
    }

    #[test]
    fn round_trips() {
        let digest = Digest::of(b"abc");
        assert_eq!(digest.to_string().parse::<Digest>().unwrap(), digest);
        assert_eq!(Digest::try_from(digest.as_bytes().as_slice()).unwrap(), digest);
        let json = serde_json::to_string(&digest).unwrap();
        assert_eq!(json, format!("\"{ABC}\""));
        assert_eq!(serde_json::from_str::<Digest>(&json).unwrap(), digest);
    }

    #[test]
    fn wrong_length_is_refused() {
        assert!(Digest::try_from(&[0u8; 31][..]).is_err());
        assert!(Digest::try_from(&[0u8; 33][..]).is_err());
        assert!("abcd".parse::<Digest>().is_err());
        assert!(format!("{ABC}0").parse::<Digest>().is_err());
    }

    #[test]
    fn uppercase_is_refused() {
        assert!(ABC.to_uppercase().parse::<Digest>().is_err());
    }
}
