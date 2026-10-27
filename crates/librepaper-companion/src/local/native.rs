//! The render log: what a render's subprocess wrote, as one bounded byte
//! string.
//!
//! Every subprocess here runs through `librepaper_base::process::supervise`,
//! which drains both pipes for the process's whole lifetime and kills the
//! whole process group on a deadline or a cancel. A render keeps the tail of
//! each stream, because a failing build says what went wrong at the end. A
//! probe and Git keep the head, because an answer starts at the top.
//!
//! The callers are the render pipeline (`render`), the tool probe (`tools`)
//! and Git (`git`).

use librepaper_base::process::Run;

/// Both streams of `run` in one log, stdout first, bounded to the last
/// `MAX_LOG_BYTES`.
pub(crate) fn log_of(run: &Run) -> Vec<u8> {
    let mut log = Vec::with_capacity(run.stdout.len() + run.stderr.len());
    log.extend_from_slice(&run.stdout);
    log.extend_from_slice(&run.stderr);
    truncate_tail(log, super::protocol::MAX_LOG_BYTES)
}

/// The tail of a log/byte stream, bounded to `limit` bytes. The cut is made
/// on a byte index, which for text may land inside a character; callers
/// decode lossily, so a split character becomes a replacement character
/// rather than a panic.
fn truncate_tail(bytes: Vec<u8>, limit: usize) -> Vec<u8> {
    if bytes.len() <= limit {
        return bytes;
    }
    let start = bytes.len() - limit;
    bytes[start..].to_vec()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A render reads its log through this supervisor. A byte-index
    /// cut can land inside a multibyte character; the log is carried as bytes
    /// and decoded lossily for exactly that reason. Truncating the decoded
    /// `String` at the same byte index instead panics on a non-boundary, and
    /// in the render path that panic killed the only local worker.
    #[test]
    fn a_bounded_log_cut_inside_a_character_decodes_instead_of_panicking() {
        let text = "é".repeat(100);
        assert_eq!(text.len(), 200, "each character is two bytes");
        // An odd limit forces the cut between the two bytes of a character.
        let tail = truncate_tail(text.into_bytes(), 51);
        assert_eq!(tail.len(), 51);
        let decoded = String::from_utf8_lossy(&tail);
        assert!(decoded.starts_with('\u{fffd}'), "{decoded:?}");
        assert!(decoded.ends_with('é'), "{decoded:?}");
        assert_eq!(decoded.matches('é').count(), 25);
    }

    #[test]
    fn truncate_tail_keeps_the_end_and_leaves_a_short_log_untouched() {
        let short = b"hello".to_vec();
        assert_eq!(truncate_tail(short.clone(), 100), short);
        let long = vec![b'x'; 10];
        let tail = truncate_tail(long, 4);
        assert_eq!(tail, vec![b'x'; 4]);
    }
}
