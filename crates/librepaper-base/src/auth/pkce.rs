//! PKCE (RFC 7636), which every browser sign-in uses: the verifier stays in
//! the state cookie and only its S256 challenge goes to the provider.

use sha2::{Digest, Sha256};

use super::base64url;
use crate::util::{token, Encoding};

/// A PKCE verifier: 32 random bytes, base64url, which is inside the 43-128
/// characters the spec allows.
pub fn pkce_verifier() -> String {
    token::<32>(Encoding::Base64Url)
}

/// The S256 challenge for a verifier.
pub fn pkce_challenge(verifier: &str) -> String {
    base64url(&Sha256::digest(verifier.as_bytes()))
}

#[cfg(test)]
mod tests {
    use super::{pkce_challenge, pkce_verifier};

    #[test]
    fn challenge_matches_the_rfc_example() {
        assert_eq!(
            pkce_challenge("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"),
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
        );
    }

    #[test]
    fn verifiers_are_long_enough_and_fresh() {
        let first = pkce_verifier();
        assert!((43..=128).contains(&first.len()));
        assert_ne!(first, pkce_verifier());
    }
}
