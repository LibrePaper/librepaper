//! Canonical JSON (RFC 8785) and the digest of it.
//!
//! serde_json keeps object keys in insertion order here, so hashing
//! `to_string` of a map depends on the order a client sent. Every digest of a
//! JSON value goes through this module instead.

use serde_json::Value;
use sha2::{Digest, Sha256};

/// The RFC 8785 canonical encoding of `value`.
pub fn canonical_bytes(value: &Value) -> Vec<u8> {
    serde_jcs::to_vec(value).expect("a JSON value always has a canonical form")
}

/// Lower-case hex SHA-256 of the canonical encoding of `value`.
pub fn sha256_hex(value: &Value) -> String {
    hex::encode(Sha256::digest(canonical_bytes(value)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn key_order_does_not_change_the_digest() {
        let a: Value = serde_json::from_str(r#"{"a":1,"b":{"x":1,"y":2}}"#).unwrap();
        let b: Value = serde_json::from_str(r#"{"b":{"y":2,"x":1},"a":1}"#).unwrap();
        assert_eq!(sha256_hex(&a), sha256_hex(&b));
        assert_ne!(
            sha256_hex(&a),
            sha256_hex(&json!({"a":2,"b":{"x":1,"y":2}}))
        );
    }

    #[test]
    fn nested_maps_are_sorted() {
        let value: Value =
            serde_json::from_str(r#"{"z":[{"b":1,"a":2}],"a":{"d":null,"c":true}}"#).unwrap();
        assert_eq!(
            String::from_utf8(canonical_bytes(&value)).unwrap(),
            r#"{"a":{"c":true,"d":null},"z":[{"a":2,"b":1}]}"#
        );
    }
}
