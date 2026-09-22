//! Small things several modules share, and the one shape every timestamp
//! takes: RFC 3339 in UTC to the second, "2026-09-04T12:00:00Z".

use time::format_description::well_known::Rfc3339;
use time::macros::format_description;
use time::OffsetDateTime;
/// Compares two byte strings without branching on the first differing byte.
///
/// For a secret the caller is about to accept or refuse: a pairing token's
/// digest, a pairing code's challenge. Two lengths are told apart
/// immediately, which leaks only how long the value is -- and the values
/// this is used on are fixed-length digests, so that is nothing. The point
/// is that a near-miss and a wild guess take the same time, so a caller
/// cannot find a token a byte at a time.
///
/// Trimming, decoding and length validation belong to the caller: what a
/// value is supposed to look like is that caller's business, and doing it
/// here would mean one function deciding it for every caller.
pub fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.iter()
        .zip(b.iter())
        .fold(0u8, |difference, (x, y)| difference | (x ^ y))
        == 0
}

/// Strips control characters and trims to a length in characters, matching
/// what every backend has always stored.
pub fn clean(value: &str, limit: usize) -> String {
    value
        .chars()
        .filter(|&c| {
            let code = c as u32;
            !(code < 0x09
                || (0x0b..=0x0c).contains(&code)
                || (0x0e..=0x1f).contains(&code)
                || code == 0x7f)
        })
        .take(limit)
        .collect()
}

/// An ordered UUIDv7 entity identifier.
pub fn new_id() -> String {
    uuid::Uuid::now_v7().to_string()
}

/// Writes a message to stderr and exits: what every command does when it
/// cannot go on.
pub fn die(message: impl std::fmt::Display) -> ! {
    eprintln!("error: {message}");
    std::process::exit(1)
}

pub fn now_unix() -> i64 {
    OffsetDateTime::now_utc().unix_timestamp()
}

/// Unix milliseconds for persisted deadlines and timestamps.
#[cfg(test)]
pub fn now_millis() -> i64 {
    i64::try_from(OffsetDateTime::now_utc().unix_timestamp_nanos() / 1_000_000)
        .expect("current time fits in Unix milliseconds")
}

/// A first-seen mutation key. Call once per user intent and retain across retries.
#[cfg(test)]
pub fn new_request_key() -> String {
    format!(
        "v2.{}.{}",
        now_millis(),
        hex::encode(crate::auth::random_bytes(16))
    )
}

pub fn timestamp() -> String {
    format_unix(now_unix())
}

pub fn format_unix(unix: i64) -> String {
    let format = format_description!("[year]-[month]-[day]T[hour]:[minute]:[second]Z");
    OffsetDateTime::from_unix_timestamp(unix)
        .ok()
        .and_then(|at| at.format(&format).ok())
        .unwrap_or_default()
}

/// An RFC 3339 timestamp as seconds since the epoch, or None when it is not
/// one.
pub fn parse_timestamp(value: &str) -> Option<i64> {
    OffsetDateTime::parse(value, &Rfc3339)
        .ok()
        .map(|at| at.unix_timestamp())
}

/// Parse the timestamp without applying freshness: existing receipts may be replayed
/// after the first-seen admission window. Freshness belongs inside admission.
pub fn request_key_timestamp(key: &str) -> Option<i64> {
    let mut parts = key.split('.');
    if parts.next()? != "v2" {
        return None;
    }
    let issued = parts.next()?;
    if issued.is_empty()
        || (issued.len() > 1 && issued.starts_with('0'))
        || !issued.bytes().all(|byte| byte.is_ascii_digit())
    {
        return None;
    }
    let nonce = parts.next()?;
    if parts.next().is_some()
        || nonce.len() != 32
        || !nonce
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return None;
    }
    issued.parse().ok()
}
