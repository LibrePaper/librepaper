//! AWS Signature Version 4 query-string signing for S3 `GET` links.
//!
//! Pure: nothing here reads the clock or the network. The caller passes the
//! time, so tests can check the signature against AWS's published example.

use hmac::{Hmac, Mac};
use sha2::{Digest, Sha256};

type HmacSha256 = Hmac<Sha256>;

pub struct Credentials<'a> {
    pub access_key_id: &'a str,
    pub secret_access_key: &'a str,
    pub session_token: Option<&'a str>,
}

/// Returns a presigned URL for `method` on `path`.
///
/// `base` is `scheme://host[:port]` with no trailing slash. `path` is the
/// canonical, already URI-encoded absolute path (`/bucket/key`). `extra` are
/// additional query parameters that are signed along with the rest.
#[allow(clippy::too_many_arguments)]
pub fn presign(
    method: &str,
    base: &str,
    path: &str,
    region: &str,
    creds: &Credentials,
    now: time::OffsetDateTime,
    ttl_secs: u64,
    extra: &[(&str, &str)],
) -> String {
    let now = now.to_offset(time::UtcOffset::UTC);
    let date = now
        .format(time::macros::format_description!("[year][month][day]"))
        .expect("static date format");
    let amz_date = now
        .format(time::macros::format_description!(
            "[year][month][day]T[hour][minute][second]Z"
        ))
        .expect("static timestamp format");
    let scope = format!("{date}/{region}/s3/aws4_request");
    let credential = format!("{}/{scope}", creds.access_key_id);
    let expires = ttl_secs.to_string();

    let mut params: Vec<(&str, &str)> = vec![
        ("X-Amz-Algorithm", "AWS4-HMAC-SHA256"),
        ("X-Amz-Credential", credential.as_str()),
        ("X-Amz-Date", amz_date.as_str()),
        ("X-Amz-Expires", expires.as_str()),
        ("X-Amz-SignedHeaders", "host"),
    ];
    if let Some(token) = creds.session_token {
        params.push(("X-Amz-Security-Token", token));
    }
    params.extend_from_slice(extra);

    let mut encoded: Vec<(String, String)> = params
        .iter()
        .map(|(key, value)| (encode_path_segment(key), encode_path_segment(value)))
        .collect();
    encoded.sort();
    let canonical_query = encoded
        .iter()
        .map(|(key, value)| format!("{key}={value}"))
        .collect::<Vec<_>>()
        .join("&");

    let authority = base.split_once("://").map_or(base, |(_, rest)| rest);
    let host = authority.split('/').next().unwrap_or(authority);
    let canonical_request =
        format!("{method}\n{path}\n{canonical_query}\nhost:{host}\n\nhost\nUNSIGNED-PAYLOAD");
    let string_to_sign = format!(
        "AWS4-HMAC-SHA256\n{amz_date}\n{scope}\n{}",
        hex::encode(Sha256::digest(canonical_request.as_bytes()))
    );

    let k_date = hmac(
        format!("AWS4{}", creds.secret_access_key).as_bytes(),
        date.as_bytes(),
    );
    let k_region = hmac(&k_date, region.as_bytes());
    let k_service = hmac(&k_region, b"s3");
    let k_signing = hmac(&k_service, b"aws4_request");
    let signature = hex::encode(hmac(&k_signing, string_to_sign.as_bytes()));

    format!("{base}{path}?{canonical_query}&X-Amz-Signature={signature}")
}

/// Percent-encodes `s` by the RFC 3986 rule: unreserved characters
/// (`A-Z a-z 0-9 - _ . ~`) pass through, everything else becomes `%XX`.
pub fn encode_path_segment(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for byte in s.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
            out.push(char::from(byte));
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

fn hmac(key: &[u8], data: &[u8]) -> Vec<u8> {
    let mut mac = HmacSha256::new_from_slice(key).expect("HMAC accepts keys of any length");
    mac.update(data);
    mac.finalize().into_bytes().to_vec()
}

#[cfg(test)]
mod tests {
    use super::*;
    use time::macros::datetime;

    fn example_credentials(token: Option<&str>) -> Credentials<'_> {
        Credentials {
            access_key_id: "AKIAIOSFODNN7EXAMPLE",
            secret_access_key: "wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY",
            session_token: token,
        }
    }

    #[test]
    fn matches_aws_published_presigned_get_example() {
        let url = presign(
            "GET",
            "https://examplebucket.s3.amazonaws.com",
            "/test.txt",
            "us-east-1",
            &example_credentials(None),
            datetime!(2013-05-24 00:00:00 UTC),
            86400,
            &[],
        );
        assert!(url.ends_with(
            "X-Amz-Signature=aeeed9bbccd4d02ee5c0109b86d86835f995330da4c265957d157751f604d404"
        ));
        assert!(url.contains("X-Amz-Expires=86400"));
    }

    #[test]
    fn extra_parameters_and_session_token_are_signed() {
        let now = datetime!(2013-05-24 00:00:00 UTC);
        let plain = presign(
            "GET",
            "https://examplebucket.s3.amazonaws.com",
            "/test.txt",
            "us-east-1",
            &example_credentials(None),
            now,
            86400,
            &[],
        );
        let with_extra = presign(
            "GET",
            "https://examplebucket.s3.amazonaws.com",
            "/test.txt",
            "us-east-1",
            &example_credentials(Some("TOKEN")),
            now,
            86400,
            &[
                ("response-content-type", "application/octet-stream"),
                ("response-content-disposition", "attachment"),
            ],
        );
        assert!(with_extra.contains("X-Amz-Security-Token=TOKEN"));
        assert!(with_extra.contains("response-content-type=application%2Foctet-stream"));
        assert!(with_extra.contains("response-content-disposition=attachment"));
        let signature = |url: &str| url.rsplit("X-Amz-Signature=").next().unwrap().to_owned();
        assert_ne!(signature(&plain), signature(&with_extra));
    }

    #[test]
    fn encodes_path_segments_by_rfc_3986() {
        assert_eq!(encode_path_segment("AZaz09-_.~"), "AZaz09-_.~");
        assert_eq!(encode_path_segment("a b/c"), "a%20b%2Fc");
        assert_eq!(encode_path_segment("é"), "%C3%A9");
    }
}
