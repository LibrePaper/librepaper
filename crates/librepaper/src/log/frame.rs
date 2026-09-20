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
/// single batch is capped at `MAX_UPDATE_BYTES` on the way in, and a peer key
/// is an account id or a deployment marker.
const MAX_PEER_KEY: usize = 256;
const MAX_BATCH_BYTES: usize = 64 * 1024 * 1024;

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
    let mut out = Vec::with_capacity(
        1 + batches
            .iter()
            .map(|batch| 14 + batch.peer_key.len() + batch.bytes.len())
            .sum::<usize>(),
    );
    out.push(VERSION);
    for batch in batches {
        let key = batch.peer_key.as_bytes();
        out.extend_from_slice(&(key.len() as u16).to_be_bytes());
        out.extend_from_slice(key);
        out.extend_from_slice(&batch.client_seq.to_be_bytes());
        out.extend_from_slice(&(batch.bytes.len() as u32).to_be_bytes());
        out.extend_from_slice(&batch.bytes);
    }
    out
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

    #[test]
    fn an_unknown_version_is_named() {
        assert_eq!(decode(&[0x02]), Err(FrameError::UnknownVersion(2)));
        assert_eq!(decode(&[]), Err(FrameError::Empty));
    }
}
