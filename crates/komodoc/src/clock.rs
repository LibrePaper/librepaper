//! Timestamps, the one shape both the index and the room use: RFC 3339 in UTC
//! to the second, "2026-09-04T12:00:00Z".

use time::format_description::well_known::Rfc3339;
use time::macros::format_description;
use time::OffsetDateTime;

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
