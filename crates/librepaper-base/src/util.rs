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

/// `n` bytes from the operating system's random generator.
pub fn random_bytes(n: usize) -> Vec<u8> {
    use rand::RngCore;
    let mut raw = vec![0u8; n];
    rand::rng().fill_bytes(&mut raw);
    raw
}

/// Sixteen random bytes as lowercase hex.
pub fn random_token() -> String {
    hex::encode(random_bytes(16))
}

/// Unix milliseconds for persisted deadlines and timestamps.
pub fn now_millis() -> i64 {
    i64::try_from(OffsetDateTime::now_utc().unix_timestamp_nanos() / 1_000_000)
        .expect("current time fits in Unix milliseconds")
}

/// A first-seen mutation key. Call once per user intent and retain across retries.
///
/// The MCP bridge is now a caller too: it mints this on behalf of a model for
/// every mutating document tool call, in the exact shape the server's
/// `OperationKey::validate` requires, so the model never constructs one.
pub fn new_request_key() -> String {
    format!("v2.{}.{}", now_millis(), hex::encode(random_bytes(16)))
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

/// Base64, which is how a binary update travels on a JSON socket.
pub fn encode_update(bytes: &[u8]) -> String {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

pub fn decode_update(text: &str) -> Option<Vec<u8>> {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.decode(text).ok()
}

/// Total and available bytes of the filesystem holding `path`. Available is
/// what an unprivileged writer can use, not what root could.
// The statvfs fields are c_ulong on some targets and u64 on others, so the
// casts are not redundant everywhere this compiles.
#[allow(clippy::unnecessary_cast)]
pub fn disk_space(path: &std::path::Path) -> std::io::Result<(u64, u64)> {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        let cpath = std::ffi::CString::new(path.as_os_str().as_bytes()).map_err(|_| {
            std::io::Error::new(std::io::ErrorKind::InvalidInput, "path holds a NUL byte")
        })?;
        let mut stat = std::mem::MaybeUninit::<libc::statvfs>::uninit();
        // SAFETY: cpath is a valid NUL-terminated string and stat is a
        // writable out-pointer for the whole call.
        let rc = unsafe { libc::statvfs(cpath.as_ptr(), stat.as_mut_ptr()) };
        if rc != 0 {
            return Err(std::io::Error::last_os_error());
        }
        // SAFETY: statvfs returned 0, so it filled the struct.
        let stat = unsafe { stat.assume_init() };
        let frag = stat.f_frsize as u64;
        Ok((stat.f_blocks as u64 * frag, stat.f_bavail as u64 * frag))
    }
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        let wide: Vec<u16> = path
            .as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();
        let mut available = 0u64;
        let mut total = 0u64;
        let mut free = 0u64;
        // SAFETY: wide is NUL-terminated and the three out-pointers live
        // for the whole call.
        let ok = unsafe {
            windows_sys::Win32::Storage::FileSystem::GetDiskFreeSpaceExW(
                wide.as_ptr(),
                &mut available,
                &mut total,
                &mut free,
            )
        };
        if ok == 0 {
            return Err(std::io::Error::last_os_error());
        }
        Ok((total, available))
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = path;
        Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "no filesystem statistics on this platform",
        ))
    }
}
