//! Small things several modules share, and the one shape every timestamp
//! takes: RFC 3339 in UTC to the second, "2026-09-04T12:00:00Z".

use time::format_description::well_known::Rfc3339;
use time::macros::format_description;
use time::OffsetDateTime;
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

/// A random UUID v4, as crypto.randomUUID does in a browser.
pub fn new_id() -> String {
    let mut bytes = crate::auth::random_bytes(16);
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    let encoded = hex::encode(bytes);
    format!(
        "{}-{}-{}-{}-{}",
        &encoded[0..8],
        &encoded[8..12],
        &encoded[12..16],
        &encoded[16..20],
        &encoded[20..]
    )
}

/// Writes a message to stderr and exits: what every command does when it
/// cannot go on.
pub fn die(message: impl std::fmt::Display) -> ! {
    eprintln!("error: {message}");
    std::process::exit(1)
}

pub fn is_terminal_stdout() -> bool {
    use std::io::IsTerminal;
    std::io::stdout().is_terminal()
}

pub fn now_unix() -> i64 {
    OffsetDateTime::now_utc().unix_timestamp()
}

/// Unix milliseconds for all persisted v2 deadlines and timestamps.
pub fn now_millis() -> i64 {
    i64::try_from(OffsetDateTime::now_utc().unix_timestamp_nanos() / 1_000_000)
        .expect("current time fits in Unix milliseconds")
}

/// A first-seen mutation key. Call once per user intent and retain across retries.
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
