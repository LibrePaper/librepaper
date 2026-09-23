//! How a flushed row carries the batches that went into it.
//!
//! `update_bytes` is `0x01`, then repeated
//! `[peer_key_len u16][peer_key][client_seq u64][bytes_len u32][bytes]`
//! (SPEC-server-is-a-log §4.2.1). The frame carries attribution -- which peer
//! wrote which batch, under which of its own sequence numbers -- and lets
//! replay hand each batch to `LoroDoc::import_batch` rather than concatenating
//! them into one blob whose parts can no longer be told apart.
//!
//! A row is written once and never updated, so this is a read-only format
//! after the flush that produced it.

/// The only version marker there is. A row whose first byte is anything else
/// was written by something that is not this program.
pub const VERSION: u8 = 0x01;

/// Bounds on what a frame may claim, so a corrupt or hostile row cannot make
/// the decoder allocate. Both are far above anything a flush produces: a
/// peer key is an account id or a deployment marker.
pub const MAX_PEER_KEY: usize = 256;
const MAX_BATCH_BYTES: usize = 64 * 1024 * 1024;

/// What one batch's header costs, on top of its payload:
/// `[peer_key_len u16][client_seq u64][bytes_len u32]`, and then the key
/// itself. Named rather than spelled `14` in three places, because the
/// pending accounting in [`super::pending`] charges it -- a buffer of very
/// many very small updates is mostly this, and an estimate that ignored it
/// would under-reserve the row it is about to write by however many batches
/// went into it.
pub const BATCH_HEADER_BYTES: usize = 2 + 8 + 4;

/// What framing one batch from `peer_key` adds to the row.
pub fn overhead(peer_key: &str) -> usize {
    BATCH_HEADER_BYTES + peer_key.len()
}

/// What the version byte costs. A row is this plus each batch's payload and
/// [`overhead`], exactly -- which is what lets a flush reserve its scratch
/// from the buffer's own accounting rather than by encoding first and
/// measuring afterwards.
pub const ROW_HEADER_BYTES: usize = 1;

/// One batch as a client sent it, with that client's session sequence.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Batch {
    /// Who sent it: an account id, a link identity, or the deployment's own
    /// key for server-authored source (§7.3).
    pub peer_key: String,
    /// That peer's own `seq` for this batch, which is what its acknowledgement
    /// names. For server-authored source it is the command's request id, cast
    /// to a number the row can hold.
    pub client_seq: i64,
    pub bytes: Vec<u8>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FrameError {
    Empty,
    UnknownVersion(u8),
    Truncated,
    Oversized,
}

impl std::fmt::Display for FrameError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Empty => formatter.write_str("an update row holds no bytes"),
            Self::UnknownVersion(found) => {
                write!(formatter, "an update row is framed as version {found}")
            }
            Self::Truncated => formatter.write_str("an update row ends inside a batch"),
            Self::Oversized => formatter.write_str("an update row claims an impossible length"),
        }
    }
}

impl std::error::Error for FrameError {}

/// Frames a flush. The order is the order the batches were accepted in, which
/// is the order replay must import them in.
pub fn encode(batches: &[Batch]) -> Vec<u8> {
    let mut out = begin(
        batches
            .iter()
            .map(|batch| overhead(&batch.peer_key) + batch.bytes.len())
            .sum::<usize>(),
    );
    for batch in batches {
        append(&mut out, &batch.peer_key, batch.client_seq, &batch.bytes);
    }
    out
}

/// Encode borrowed batches with one allocation sized for the complete row,
/// including a command's prepared source after its buffered prefix.
pub fn encode_slices<'a>(
    batches: impl Iterator<Item = (&'a str, i64, &'a [u8])> + Clone,
) -> Vec<u8> {
    let body = batches
        .clone()
        .map(|(peer, _, bytes)| overhead(peer) + bytes.len())
        .sum();
    let mut out = begin(body);
    for (peer, seq, bytes) in batches {
        append(&mut out, peer, seq, bytes);
    }
    out
}

/// Starts a row, with room for `body` bytes of batches.
///
/// Separate from [`append`] so a flush can encode straight out of the buffer
/// it holds, without first copying every payload into a `Vec<Batch>` it would
/// then throw away. That copy used to be the largest transient allocation on
/// the persistence path, and the one nothing accounted for.
pub fn begin(body: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(ROW_HEADER_BYTES.saturating_add(body));
    out.push(VERSION);
    out
}

/// Writes one batch into a row [`begin`] started.
pub fn append(out: &mut Vec<u8>, peer_key: &str, client_seq: i64, bytes: &[u8]) {
    let key = peer_key.as_bytes();
    out.extend_from_slice(&(key.len() as u16).to_be_bytes());
    out.extend_from_slice(key);
    out.extend_from_slice(&client_seq.to_be_bytes());
    out.extend_from_slice(&(bytes.len() as u32).to_be_bytes());
    out.extend_from_slice(bytes);
}

/// Reads a row back. Every length is checked against what remains before it
/// is used, so a truncated or fabricated row is an error rather than a panic
/// or an allocation.
pub fn decode(bytes: &[u8]) -> Result<Vec<Batch>, FrameError> {
    let Some((version, mut rest)) = bytes.split_first() else {
        return Err(FrameError::Empty);
    };
    if *version != VERSION {
        return Err(FrameError::UnknownVersion(*version));
    }
    let mut batches = Vec::new();
    while !rest.is_empty() {
        let key_len = usize::from(take_u16(&mut rest)?);
        if key_len > MAX_PEER_KEY {
            return Err(FrameError::Oversized);
        }
        let key = take(&mut rest, key_len)?;
        let client_seq = take_i64(&mut rest)?;
        let batch_len = take_u32(&mut rest)? as usize;
        if batch_len > MAX_BATCH_BYTES {
            return Err(FrameError::Oversized);
        }
        let payload = take(&mut rest, batch_len)?;
        batches.push(Batch {
            // A peer key that is not UTF-8 was not written here. Attribution
            // is advisory, so it is replaced rather than made fatal: the
            // operations in the batch carry the authorship that matters.
            peer_key: String::from_utf8_lossy(key).into_owned(),
            client_seq,
            bytes: payload.to_vec(),
        });
    }
    Ok(batches)
}

fn take<'a>(rest: &mut &'a [u8], count: usize) -> Result<&'a [u8], FrameError> {
    if rest.len() < count {
        return Err(FrameError::Truncated);
    }
    let (head, tail) = rest.split_at(count);
    *rest = tail;
    Ok(head)
}

fn take_u16(rest: &mut &[u8]) -> Result<u16, FrameError> {
    let bytes = take(rest, 2)?;
    Ok(u16::from_be_bytes([bytes[0], bytes[1]]))
}

fn take_u32(rest: &mut &[u8]) -> Result<u32, FrameError> {
    let bytes = take(rest, 4)?;
    Ok(u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
}

fn take_i64(rest: &mut &[u8]) -> Result<i64, FrameError> {
    let bytes = take(rest, 8)?;
    let mut buffer = [0u8; 8];
    buffer.copy_from_slice(bytes);
    Ok(i64::from_be_bytes(buffer))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn batch(key: &str, seq: i64, bytes: &[u8]) -> Batch {
        Batch {
            peer_key: key.to_string(),
            client_seq: seq,
            bytes: bytes.to_vec(),
        }
    }

    #[test]
    fn a_row_round_trips_with_its_attribution() {
        let batches = vec![
            batch("account:1", 7, b"first"),
            batch("", -1, b""),
            batch("deployment", i64::MAX, &[0xff; 300]),
        ];
        assert_eq!(decode(&encode(&batches)).unwrap(), batches);
    }

    #[test]
    fn an_empty_row_decodes_to_no_batches() {
        assert_eq!(decode(&encode(&[])).unwrap(), Vec::new());
    }

    #[test]
    fn a_truncated_row_is_an_error_and_not_a_panic() {
        let encoded = encode(&[batch("a", 1, b"body")]);
        // From 2, not from 1: the version byte on its own is a complete row
        // that happens to hold no batches, which is what `encode(&[])`
        // produces and what the case below pins. Every cut after it lands
        // inside a batch's header or its payload, and each of those has to
        // be an error rather than a partial decode -- a row is read back by
        // replay, and a replay that silently dropped the tail of a row would
        // rebuild a document missing somebody's work.
        for cut in 2..encoded.len() {
            assert!(
                decode(&encoded[..cut]).is_err(),
                "a row cut at {cut} decoded as if it were whole"
            );
        }
        assert_eq!(
            decode(&encoded[..1]),
            Ok(Vec::new()),
            "the version byte alone is an empty row, not a truncated one"
        );
    }

    #[test]
    fn a_fabricated_length_is_refused_before_it_allocates() {
        // version, key length 0, seq, then a batch length of 4 GiB.
        let mut bytes = vec![VERSION, 0, 0];
        bytes.extend_from_slice(&1i64.to_be_bytes());
        bytes.extend_from_slice(&u32::MAX.to_be_bytes());
        assert_eq!(decode(&bytes), Err(FrameError::Oversized));
    }

    /// The pending accounting reserves a row's scratch from the buffer's own
    /// charges rather than by encoding first and measuring afterwards, so
    /// "one version byte plus payload plus `overhead` per batch" has to be
    /// the exact encoded length and not an approximation of it.
    #[test]
    fn a_rows_length_is_exactly_what_the_overhead_helpers_predict() {
        let batches = vec![
            batch("account:1", 7, b"first"),
            batch("", -1, b""),
            batch("deployment", i64::MAX, &[0xff; 300]),
        ];
        let predicted = ROW_HEADER_BYTES
            + batches
                .iter()
                .map(|one| overhead(&one.peer_key) + one.bytes.len())
                .sum::<usize>();
        assert_eq!(encode(&batches).len(), predicted);
        assert_eq!(encode(&[]).len(), ROW_HEADER_BYTES);
    }

    /// `begin`/`append` is what a flush uses instead of building a
    /// `Vec<Batch>` it would throw away, so it has to produce the same bytes.
    #[test]
    fn appending_batches_one_at_a_time_produces_the_same_row() {
        let batches = vec![batch("account:1", 7, b"first"), batch("b", 2, b"second")];
        let mut piecewise = begin(0);
        for one in &batches {
            append(&mut piecewise, &one.peer_key, one.client_seq, &one.bytes);
        }
        assert_eq!(piecewise, encode(&batches));
    }

    #[test]
    fn an_unknown_version_is_named() {
        assert_eq!(decode(&[0x02]), Err(FrameError::UnknownVersion(2)));
        assert_eq!(decode(&[]), Err(FrameError::Empty));
    }
    #[test]
    fn prepared_source_does_not_double_a_large_prefix_allocation() {
        let prefix = vec![1u8; 2 * 1024 * 1024];
        let prepared = vec![2u8; 128];
        let batches = [
            ("a", 1, prefix.as_slice()),
            ("deployment", 2, prepared.as_slice()),
        ];
        let encoded = encode_slices(batches.into_iter());
        let expected = ROW_HEADER_BYTES
            + overhead("a")
            + prefix.len()
            + overhead("deployment")
            + prepared.len();
        assert_eq!(encoded.len(), expected);
        assert_eq!(
            encoded.capacity(),
            expected,
            "the full row must be allocated once"
        );
        assert_eq!(decode(&encoded).expect("round trip")[1].bytes, prepared);
    }
}
