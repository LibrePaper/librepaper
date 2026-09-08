//! Small things several modules share, and the one shape every timestamp
//! takes: RFC 3339 in UTC to the second, "2026-09-04T12:00:00Z".

use time::format_description::well_known::Rfc3339;
use time::macros::format_description;
use time::OffsetDateTime;
/// The first value that is not empty once unquoted.
pub fn first_of(values: &[&str]) -> String {
    for value in values {
        let cleaned = unquote(value);
        if !cleaned.is_empty() {
            return cleaned;
        }
    }
    String::new()
}

/// Drops surrounding quotes. A .env read by make keeps them, unlike a shell,
/// and a client id wearing quotation marks is one GitHub has never heard of.
/// Every other .env convention allows them, so accept them here.
pub fn unquote(value: &str) -> String {
    let trimmed = value.trim();
    let bytes = trimmed.as_bytes();
    if bytes.len() >= 2 {
        let (first, last) = (bytes[0], bytes[bytes.len() - 1]);
        if (first == b'"' && last == b'"') || (first == b'\'' && last == b'\'') {
            return trimmed[1..trimmed.len() - 1].trim().to_string();
        }
    }
    trimmed.to_string()
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

pub fn is_terminal_stdin() -> bool {
    use std::io::IsTerminal;
    std::io::stdin().is_terminal()
}

/// Reads one line from standard input, trimmed.
pub fn read_line() -> String {
    let mut line = String::new();
    let _ = std::io::stdin().read_line(&mut line);
    line.trim().to_string()
}

pub fn now_unix() -> i64 {
    OffsetDateTime::now_utc().unix_timestamp()
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

/// The date and time SigV4 wants: "20260904T120000Z" and "20260904".
pub fn amz_stamps(unix: i64) -> (String, String) {
    let stamp = format_description!("[year][month][day]T[hour][minute][second]Z");
    let day = format_description!("[year][month][day]");
    let at = OffsetDateTime::from_unix_timestamp(unix).unwrap_or(OffsetDateTime::UNIX_EPOCH);
    (
        at.format(&stamp).unwrap_or_default(),
        at.format(&day).unwrap_or_default(),
    )
}
